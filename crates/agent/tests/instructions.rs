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

fn typed_instruction_messages(events: &[session::Event]) -> Vec<model::Message> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. }
                if matches!(
                    message.source,
                    model::MessageSource::AgentInstructions { .. }
                ) =>
            {
                Some(message.clone())
            }
            _ => None,
        })
        .collect()
}

/// T006.10：基线注入消息带 DSH 类型来源（agent-instructions）、baseline identity 与
/// set 变更对象（scope/path/digest），并写进会话日志（续接可从日志对账）。
#[tokio::test]
async fn baseline_message_has_typed_source_and_baseline_identity() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "TYPED-INSTRUCTION\n").unwrap();
    let (agent, data_dir, session_id) = setup_agent_custom(
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

    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let messages = typed_instruction_messages(store.events());
    assert_eq!(messages.len(), 1, "应有且只有一条基线注入消息");
    let model::MessageSource::AgentInstructions {
        form,
        baseline,
        baseline_identity,
        changes,
    } = &messages[0].source
    else {
        panic!("source 应是 agent-instructions");
    };
    assert_eq!(form, "instructions");
    assert!(*baseline, "第一条是完整基线");
    let identity = baseline_identity.as_deref().expect("基线带 identity");
    assert!(!identity.is_empty());
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].action, model::InstructionChangeAction::Set);
    assert!(changes[0].scope.contains("AGENTS.md"), "scope 带候选文件名");
    assert_eq!(changes[0].path, "AGENTS.md", "项目路径是项目根相对路径");
    let digest = changes[0].digest.as_deref().expect("set 带内容 digest");
    assert_eq!(digest.len(), 16, "FNV-1a 16 hex（DSH SHA-1 的等价适配）");
    assert!(
        digest
            .chars()
            .all(|character| character.is_ascii_hexdigit())
    );
    // 注入文本也进派生请求（普通历史），内容来自文件。
    assert!(messages[0].content.iter().any(|block| {
        block
            .as_text()
            .is_some_and(|text| text.contains("TYPED-INSTRUCTION"))
    }));
}

/// T006.10：文件内容变化产生 replace 注入、文件消失产生 remove 注入（照 DSH
/// renderInstructionChanges 的 Updated / Instructions removed 文案）；会话日志里的变更对象
/// 可用于续接对账。
#[tokio::test]
async fn changed_and_removed_files_produce_replace_and_remove_messages() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "CHANGE-V1\n").unwrap();
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

    std::fs::write(project.path().join("AGENTS.md"), "CHANGE-V2\n").unwrap();
    agent
        .send(UserMessage::text("two"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    std::fs::remove_file(project.path().join("AGENTS.md")).unwrap();
    agent
        .send(UserMessage::text("three"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8(request.body.clone()).unwrap())
        .collect();
    assert!(
        bodies[1].contains("Updated instructions from: AGENTS.md")
            && bodies[1].contains(
                "Use the following content instead of the previously loaded instructions"
            ),
        "内容变化要发 replace 注入：{}",
        bodies[1]
    );
    assert!(bodies[1].contains("CHANGE-V2"), "replace 带新内容");
    // 请求历史里仍有基线消息（CHANGE-V1），这里只验证新的 replace 注入带新内容。
    let updated_at = bodies[1]
        .find("Updated instructions from: AGENTS.md")
        .unwrap();
    let updated = &bodies[1][updated_at..];
    assert!(updated.contains("CHANGE-V2"));
    assert!(!updated[..updated.len().min(400)].contains("CHANGE-V1"));
    assert!(
        bodies[2].contains("Instructions removed: AGENTS.md")
            && bodies[2].contains("no longer apply"),
        "文件消失要发 remove 注入：{}",
        bodies[2]
    );

    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let messages = typed_instruction_messages(store.events());
    let actions: Vec<model::InstructionChangeAction> = messages
        .iter()
        .filter_map(|message| match &message.source {
            model::MessageSource::AgentInstructions { changes, .. } => {
                Some(changes.iter().map(|change| change.action))
            }
            _ => None,
        })
        .flatten()
        .collect();
    assert_eq!(
        actions,
        vec![
            model::InstructionChangeAction::Set,
            model::InstructionChangeAction::Replace,
            model::InstructionChangeAction::Remove
        ],
        "日志里的变更对象依次是 set/replace/remove"
    );
}

/// T006.10：续接对账——文件没变不重复注入基线；恢复后文件变了会发 replace 注入。
#[tokio::test]
async fn resume_reconciles_instead_of_reinjecting_baseline() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    std::fs::write(project.path().join("AGENTS.md"), "RESUME-V1\n").unwrap();
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
    drop(agent);

    let store = session::SessionStore::open(data_dir.path(), &session_id).unwrap();
    let config = agent::AgentConfig {
        instruction_roots: Vec::new(),
        ..common::isolated_config(data_dir.path())
    };
    let agent = agent::Agent::resume(
        session_id.clone(),
        config,
        agent::Deps {
            model: model::DeepSeek::new(common::model_config(&server.uri())).unwrap(),
            tools: ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("resume-same"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    {
        let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
        let baselines = typed_instruction_messages(store.events())
            .into_iter()
            .filter(|message| {
                matches!(
                    &message.source,
                    model::MessageSource::AgentInstructions { baseline: true, .. }
                )
            })
            .count();
        assert_eq!(baselines, 1, "续接后未变化的基线不重复注入");
    }

    std::fs::write(project.path().join("AGENTS.md"), "RESUME-V2\n").unwrap();
    agent
        .send(UserMessage::text("resume-changed"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    let last = String::from_utf8(requests.last().unwrap().body.clone()).unwrap();
    assert!(
        last.contains("Updated instructions from: AGENTS.md") && last.contains("RESUME-V2"),
        "续接后文件变化要发 replace 注入：{last}"
    );
}

/// T006.10：异步读取单文件上限（照 DSH maxSourceBytes）——超大文件跳过，不 panic、不注入。
#[tokio::test]
async fn oversized_instruction_source_is_skipped() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;

    let project = tempfile::tempdir().unwrap();
    std::fs::create_dir(project.path().join(".git")).unwrap();
    let oversized = vec![b'x'; (agent::MAX_SOURCE_BYTES as usize) + 1];
    std::fs::write(project.path().join("AGENTS.md"), oversized).unwrap();
    let (agent, data_dir, session_id) = setup_agent_custom(
        &server.uri(),
        ToolSet::builtin(),
        project.path(),
        Vec::new(),
    )
    .await;
    agent
        .send(UserMessage::text("看大文件"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let store = session::SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert!(
        typed_instruction_messages(store.events()).is_empty(),
        "超过 maxSourceBytes 的文件被跳过，不产生空/大的注入消息"
    );
    let requests = server.received_requests().await.unwrap();
    let first = String::from_utf8(requests[0].body.clone()).unwrap();
    assert!(!first.contains("Instructions from:"), "超大文件不进请求");
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
