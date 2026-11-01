//! 接缝 1：skill 目录进入请求（测试清单第 6 条）。

mod common;

use agent::Delivery;
use common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent_custom, setup_agent_with_roots, sse};
use model::UserMessage;
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn write(path: &std::path::Path, content: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, content).unwrap();
}

async fn mount_final(server: &MockServer) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(server)
        .await;
}

#[tokio::test]
async fn first_request_contains_skill_catalog() {
    let server = MockServer::start().await;
    mount_final(&server).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/pdf-tools/SKILL.md"),
        "---\nname: pdf-tools\ndescription: Handle PDF files.\n---\n# PDF tools\nUse the pdf command.\n",
    );
    let (agent, _data_dir, _id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;

    agent.send(UserMessage::text("看看 skill"), Delivery::FollowUp);
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(
        first.contains("<available_skills>"),
        "第一次请求应带 skill 目录：{first}"
    );
    assert!(
        first.contains("- `pdf-tools`:"),
        "目录条目应有名字：{first}"
    );
    assert!(
        first.contains("Handle PDF files."),
        "目录条目应有 description：{first}"
    );
    assert_eq!(
        first.matches("<available_skills>").count(),
        1,
        "目录只出现一次"
    );
    // 目录只是摘要：正文不应在请求里。
    assert!(
        !first.contains("Use the pdf command."),
        "未加载前不应注入正文"
    );
}

#[tokio::test]
async fn catalog_lists_user_and_configured_roots_with_project_shadowing() {
    let server = MockServer::start().await;
    mount_final(&server).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/dup/SKILL.md"),
        "---\nname: dup\ndescription: PROJECT-DESC.\n---\nProject body.\n",
    );
    let agents_home = tempfile::tempdir().unwrap();
    write(
        &agents_home.path().join("baz/SKILL.md"),
        "---\nname: baz\ndescription: BAZ-DESC.\n---\nBaz body.\n",
    );
    let (agent, data_dir, _id) = setup_agent_with_roots(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        vec![agents_home.path().to_path_buf()],
    )
    .await;
    // 用户根再加一个同名 dup（应被项目根遮蔽）和一个 bar。
    write(
        &data_dir.path().join("skills/dup/SKILL.md"),
        "---\nname: dup\ndescription: USER-DESC.\n---\nUser body.\n",
    );
    write(
        &data_dir.path().join("skills/bar/SKILL.md"),
        "---\nname: bar\ndescription: BAR-DESC.\n---\nBar body.\n",
    );

    agent.send(UserMessage::text("看目录"), Delivery::FollowUp);
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(first.contains("- `baz`:"), "配置根 skill 应进目录");
    assert!(first.contains("- `bar`:"), "data-dir 用户根 skill 应进目录");
    assert!(first.contains("PROJECT-DESC"), "项目根同名 skill 胜出");
    assert!(!first.contains("USER-DESC"), "用户根同名 skill 不应出现");
}

#[tokio::test]
async fn no_skills_means_no_catalog_message() {
    let server = MockServer::start().await;
    mount_final(&server).await;
    let cwd = tempfile::tempdir().unwrap();
    let (agent, _data_dir, _id) =
        setup_agent_custom(&server.uri(), ToolSet::builtin(), cwd.path(), Vec::new()).await;
    agent.send(UserMessage::text("没有 skill"), Delivery::FollowUp);
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(
        !first.contains("<available_skills>"),
        "没有 skill 时不应注入目录"
    );
}
