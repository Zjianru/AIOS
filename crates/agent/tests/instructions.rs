//! 接缝 1：指令文件（AGENTS.md / CLAUDE.md）的发现、注入与刷新（测试清单第 5 条）。

mod common;

use agent::Delivery;
use common::{
    FINAL_TEXT_SSE, SequenceResponder, model_config, setup_agent_custom, sse,
    tool_calls_with_inputs_sse,
};
use model::UserMessage;
use serde_json::json;
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

#[tokio::test]
async fn first_request_includes_root_to_cwd_chain() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "ROOT-INSTRUCTION\n").unwrap();
    let sub = project.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("AGENTS.md"), "SUB-INSTRUCTION\n").unwrap();
    let agents_root = tempfile::tempdir().unwrap();
    std::fs::write(agents_root.path().join("AGENTS.md"), "USER-INSTRUCTION\n").unwrap();

    let (agent, data_dir, _id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        &sub,
        vec![agents_root.path().to_path_buf()],
    )
    .await;
    // 用户级根还包括 <data-dir>（由 agent 固定加入）。
    std::fs::write(data_dir.path().join("AGENTS.md"), "DATA-INSTRUCTION\n").unwrap();

    agent
        .send(UserMessage::text("看一下指令"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert!(!requests.is_empty());
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    for marker in [
        "DATA-INSTRUCTION",
        "USER-INSTRUCTION",
        "ROOT-INSTRUCTION",
        "SUB-INSTRUCTION",
    ] {
        assert!(first.contains(marker), "第一次请求缺 {marker}");
    }
    // 顺序：用户全局（data-dir → ~/.agents）在前，项目链从根到 cwd。
    let data = first.find("DATA-INSTRUCTION").unwrap();
    let user = first.find("USER-INSTRUCTION").unwrap();
    let root = first.find("ROOT-INSTRUCTION").unwrap();
    let sub_at = first.find("SUB-INSTRUCTION").unwrap();
    assert!(data < user && user < root && root < sub_at, "指令顺序不对");
}

#[tokio::test]
async fn deeper_directory_instructions_added_on_next_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "read",
                json!({"file_path": "deep/file.txt"}),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let deep = project.path().join("deep");
    std::fs::create_dir(&deep).unwrap();
    std::fs::write(deep.join("AGENTS.md"), "DEEP-INSTRUCTION\n").unwrap();
    std::fs::write(deep.join("file.txt"), "hello\n").unwrap();

    let (agent, _data_dir, _id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;

    agent
        .send(UserMessage::text("读一下深目录"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "应有两次请求");
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(
        !first.contains("DEEP-INSTRUCTION"),
        "第一次请求不该含深目录指令"
    );
    assert!(
        second.contains("DEEP-INSTRUCTION"),
        "读到深目录后下一次请求应补入该目录指令"
    );
}

#[tokio::test]
async fn duplicate_instruction_content_rendered_once() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    // CLAUDE.md 全量复制 AGENTS.md：同一候选组内只渲染一次。
    std::fs::write(project.path().join("AGENTS.md"), "SAME-INSTRUCTION\n").unwrap();
    std::fs::write(project.path().join("CLAUDE.md"), "SAME-INSTRUCTION\n").unwrap();

    let (agent, _data_dir, _id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;

    agent
        .send(UserMessage::text("看指令"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert_eq!(
        first.matches("SAME-INSTRUCTION").count(),
        1,
        "相同内容只应出现一次"
    );
    assert_eq!(
        first.matches("Instructions from:").count(),
        1,
        "只应有一个指令段"
    );
}

const NO_HOME_CHILD_ENV: &str = "AIOS_TEST_NO_HOME_CHILD";

#[tokio::test]
async fn default_config_without_home_is_valid() {
    // 父进程里去掉 HOME，再跑本用例：默认配置的根必须绝对或为空，不能建 agent 就报错。
    if std::env::var_os(NO_HOME_CHILD_ENV).is_none() {
        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new(&exe)
            .arg("default_config_without_home_is_valid")
            .arg("--exact")
            .arg("--nocapture")
            .env_remove("HOME")
            .env(NO_HOME_CHILD_ENV, "1")
            .env("AIOS_TEST_BASE_URL", "http://127.0.0.1:9")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "无 HOME 时默认配置不可用\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }

    let config = agent::AgentConfig::default();
    assert!(
        config
            .instruction_roots
            .iter()
            .all(|root| root.is_absolute()),
        "无 HOME 时指令根必须是绝对路径或空：{:?}",
        config.instruction_roots
    );
    assert!(
        config.skill_roots.iter().all(|root| root.is_absolute()),
        "无 HOME 时 skill 根必须是绝对路径或空：{:?}",
        config.skill_roots
    );
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let base_url = std::env::var("AIOS_TEST_BASE_URL").unwrap_or_default();
    let created = agent::Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::new(model_config(&base_url)).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await;
    assert!(
        created.is_ok(),
        "无 HOME 时默认配置建 agent 失败：{:?}",
        created.err()
    );
}
