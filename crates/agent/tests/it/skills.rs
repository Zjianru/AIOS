//! 接缝 1：skill 目录进入请求（测试清单第 6 条）。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, setup_agent_custom, setup_agent_with_roots, sse,
    tool_calls_with_inputs_sse,
};
use agent::Delivery;
use model::UserMessage;
use serde_json::json;
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

    agent
        .send(UserMessage::text("看看 skill"), Delivery::FollowUp)
        .await
        .unwrap();
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

    agent
        .send(UserMessage::text("看目录"), Delivery::FollowUp)
        .await
        .unwrap();
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
    agent
        .send(UserMessage::text("没有 skill"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(
        !first.contains("<available_skills>"),
        "没有 skill 时不应注入目录"
    );
}

fn typed_catalog_messages(events: &[session::Event]) -> Vec<model::Message> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. }
                if matches!(message.source, model::MessageSource::SkillCatalog { .. }) =>
            {
                Some(message.clone())
            }
            _ => None,
        })
        .collect()
}

/// T006.11：目录消息带 DSH 类型来源（skill-catalog：form/update/entries），第二次发布是
/// 「替换目录」协议（update=true + 完整新条目 + DSH replacement 文案）。
#[tokio::test]
async fn catalog_message_has_typed_source_and_replacement_protocol() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: Alpha desc.\n---\nAlpha body.\n",
    );
    let (agent, data_dir, session_id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(UserMessage::text("one"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    write(
        &project.path().join(".agents/skills/beta/SKILL.md"),
        "---\nname: beta\ndescription: Beta desc.\n---\nBeta body.\n",
    );
    agent
        .send(UserMessage::text("two"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let messages = typed_catalog_messages(store.events());
    assert_eq!(messages.len(), 2, "首次发布 + 替换目录各一条");
    let model::MessageSource::SkillCatalog {
        form,
        update,
        incomplete,
        entries,
    } = &messages[0].source
    else {
        panic!("source 应是 skill-catalog");
    };
    assert_eq!(form, "catalog");
    assert!(
        !update && !incomplete,
        "首次发布：update=false/incomplete=false"
    );
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "alpha");
    assert_eq!(entries[0].description, "Alpha desc.");
    let model::MessageSource::SkillCatalog {
        update, entries, ..
    } = &messages[1].source
    else {
        panic!("source 应是 skill-catalog");
    };
    assert!(update, "第二次发布是替换目录");
    assert_eq!(
        entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "beta"]
    );

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    assert!(
        bodies[1].contains(
            "The available skill catalog changed. This complete catalog replaces every earlier available-skills list in this session:"
        ),
        "替换目录文案照 DSH：{}",
        bodies[1]
    );
    assert!(
        !bodies[0].contains("The available skill catalog changed."),
        "首次发布不是替换文案"
    );
}

/// T006.11：disable-model-invocation: true 的 skill 不进目录，skill 工具也拒绝加载
/// （DSH 的 modelInvocable=false 语义）；非法 frontmatter 不再静默丢（warning 由 tools 单测钉）。
#[tokio::test]
async fn disable_model_invocation_skill_is_hidden_and_tool_rejects() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_with_inputs_sse(&[(
                "call_0",
                "skill",
                json!({"name": "hidden"}),
            )])),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/hidden/SKILL.md"),
        "---\nname: hidden\ndescription: Hidden desc.\ndisable-model-invocation: true\n---\nHidden body.\n",
    );
    let (agent, _data_dir, _id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(UserMessage::text("试试隐藏 skill"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    assert!(
        !bodies[0].contains("Hidden desc."),
        "模型不可调用的 skill 不进目录：{}",
        bodies[0]
    );
    assert!(
        bodies[1].contains("not available for model invocation"),
        "skill 工具应拒绝模型不可调用的 skill：{}",
        bodies[1]
    );
}

fn skill_invocation_messages(events: &[session::Event]) -> Vec<model::Message> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. }
                if matches!(message.source, model::MessageSource::SkillInvocation { .. }) =>
            {
                Some(message.clone())
            }
            _ => None,
        })
        .collect()
}

/// T006.11b：用户消息里的 /name 命中 user-invocable skill 时，DSH 的 skill-invocation
/// 注入通道把全文作为一条 user 消息加进这一步（source=skill-invocation）；目录仍在前。
#[tokio::test]
async fn user_name_invocation_injects_skill_content() {
    let server = MockServer::start().await;
    mount_final(&server).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/pdf-tools/SKILL.md"),
        "---
name: pdf-tools
description: Handle PDF files.
---
# PDF tools
Use the pdf command.
",
    );
    let (agent, data_dir, session_id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(
            UserMessage::text("请用 /pdf-tools 处理这个文件"),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    let catalog_at = first.find("<available_skills>").expect("目录应在请求里");
    let skill_at = first
        .find("skill_content name=\\\"pdf-tools\\\"")
        .unwrap_or_else(|| panic!("显式 /name 应注入 skill 全文：{first}"));
    assert!(
        skill_at > catalog_at,
        "DSH 注入顺序：目录在前，用户点名的 skill 正文最后：{first}"
    );
    assert!(
        first.contains("Use the pdf command."),
        "注入的是全文：{first}"
    );

    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let messages = skill_invocation_messages(store.events());
    assert_eq!(messages.len(), 1, "每条 /name 注入一条 user 消息");
    let model::MessageSource::SkillInvocation { name, form } = &messages[0].source else {
        panic!("source 应是 skill-invocation");
    };
    assert_eq!(name, "pdf-tools");
    assert_eq!(form, "instructions");
    assert!(
        messages[0].content.iter().any(|block| matches!(
            block,
            model::ContentBlock::Text { text } if text.contains("<skill_content name=\"pdf-tools\">")
        )),
        "注入消息内容应是 renderSkillContent"
    );
}

/// T006.11b：disable-model-invocation 的 skill 不进目录、skill 工具拒绝，
/// 但用户显式 /name 是 DSH 的唯一入口，应照常注入全文。
#[tokio::test]
async fn disable_model_invocation_skill_loads_only_via_user_name() {
    let server = MockServer::start().await;
    mount_final(&server).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/hidden/SKILL.md"),
        "---
name: hidden
description: Hidden desc.
disable-model-invocation: true
---
Hidden body.
",
    );
    let (agent, data_dir, session_id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(UserMessage::text("/hidden 做事"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(
        !first.contains("<available_skills>"),
        "模型不可见 skill 不发布目录：{first}"
    );
    assert!(
        first.contains("skill_content name=\\\"hidden\\\"") && first.contains("Hidden body."),
        "用户显式 /name 应注入隐藏 skill 全文：{first}"
    );
    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        skill_invocation_messages(store.events()).len(),
        1,
        "注入一条 skill-invocation 消息"
    );
}

/// T006.11b：user-invocable=false 或未知名字的 /name 保持普通文本（DSH：静默跳过）。
#[tokio::test]
async fn non_user_invocable_or_unknown_name_stays_prose() {
    let server = MockServer::start().await;
    mount_final(&server).await;
    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    write(
        &project.path().join(".agents/skills/quiet/SKILL.md"),
        "---
name: quiet
description: Quiet desc.
user-invocable: false
---
Quiet body.
",
    );
    let (agent, data_dir, session_id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(
            UserMessage::text("/quiet 和 /nope 都不该注入"),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(!first.contains("Quiet body."), "不得注入：{first}");
    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert!(
        skill_invocation_messages(store.events()).is_empty(),
        "没有 skill-invocation 消息"
    );
}
