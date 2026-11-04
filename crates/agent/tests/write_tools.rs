//! 接缝 1：写类工具经由 Agent 驱动（测试清单第 1、2 条 + D5 加项）。

mod common;

use std::fs;

use agent::Delivery;
use common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse, tool_calls_with_inputs_sse};
use model::UserMessage;
use serde_json::json;
use session::{EventKind, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn mount(server: &MockServer, bodies: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            bodies.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

/// 会话日志里某个 call_id 的 tool/result：(is_error, error code)。
fn tool_result(store: &session::SessionSnapshot, call_id: &str) -> (bool, Option<String>) {
    for event in store.events() {
        let EventKind::ToolResult { message, error, .. } = &event.kind else {
            continue;
        };
        if message.tool_call_id.as_deref() == Some(call_id) {
            return (
                message.is_error.unwrap_or(false),
                error.as_ref().map(|error| error.code.clone()),
            );
        }
    }
    panic!("日志里没有 call_id {call_id} 的 tool/result");
}

#[tokio::test]
async fn read_then_write_across_steps_succeeds() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_0", "read", json!({"file_path": "a.txt"}))]),
            tool_calls_with_inputs_sse(&[(
                "call_1",
                "write",
                json!({"file_path": "a.txt", "content": "new\n"}),
            )]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, data_dir, cwd, id) =
        setup_agent(&server.uri(), ToolSet::builtin_with_patch()).await;
    fs::write(cwd.path().join("a.txt"), "old\n").unwrap();

    agent
        .send(UserMessage::text("先读再写"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let first: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let system = first["system"].as_str().unwrap_or_default();
    assert!(
        system.contains("re-read affected files before retrying"),
        "apply_patch 指导句缺重试建议：{system}"
    );

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(tool_result(&store, "call_0"), (false, None));
    assert_eq!(tool_result(&store, "call_1"), (false, None));
    assert_eq!(
        fs::read_to_string(cwd.path().join("a.txt")).unwrap(),
        "new\n"
    );
}

#[tokio::test]
async fn batch_write_before_read_is_rejected() {
    // D5（票面加项）：同一批里写在前、读在后，写拿不到观察，被拒；读照常成功。
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[
                (
                    "call_0",
                    "write",
                    json!({"file_path": "a.txt", "content": "new\n"}),
                ),
                ("call_1", "read", json!({"file_path": "a.txt"})),
            ]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, data_dir, cwd, id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    fs::write(cwd.path().join("a.txt"), "old\n").unwrap();

    agent
        .send(UserMessage::text("写再读"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        tool_result(&store, "call_0"),
        (true, Some("FS_NOT_OBSERVED".to_owned()))
    );
    assert_eq!(tool_result(&store, "call_1"), (false, None));
    assert_eq!(
        fs::read_to_string(cwd.path().join("a.txt")).unwrap(),
        "old\n"
    );
}

#[tokio::test]
async fn write_only_system_prompt_has_no_edit_advice() {
    let server = MockServer::start().await;
    mount(&server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let tools = ToolSet::new(vec![Box::new(tools::WriteTool::new())]);
    let (agent, _data_dir, _cwd, _id) = setup_agent(&server.uri(), tools).await;

    agent
        .send(UserMessage::text("hi"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let system = body["system"].as_str().unwrap_or_default();
    assert!(
        system.contains("Read an existing file before overwriting it with write"),
        "write 指导句缺失：{system}"
    );
    assert!(
        !system.contains("prefer edit"),
        "edit 不可见时不建议 edit：{system}"
    );
    assert!(
        !system.contains("Use the read tool"),
        "read 不可见时不提 read：{system}"
    );
    assert!(
        !system.contains("apply_patch"),
        "apply_patch 不可见时不提补丁：{system}"
    );
}

#[tokio::test]
async fn edit_after_read_across_steps_succeeds() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_0", "read", json!({"file_path": "a.txt"}))]),
            tool_calls_with_inputs_sse(&[(
                "call_1",
                "edit",
                json!({"file_path": "a.txt", "old_string": "old", "new_string": "new"}),
            )]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, data_dir, cwd, id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    fs::write(cwd.path().join("a.txt"), "old\n").unwrap();

    agent
        .send(UserMessage::text("先读再改"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(tool_result(&store, "call_0"), (false, None));
    assert_eq!(tool_result(&store, "call_1"), (false, None));
    assert_eq!(
        fs::read_to_string(cwd.path().join("a.txt")).unwrap(),
        "new\n"
    );
}

#[tokio::test]
async fn patch_after_read_across_steps_succeeds() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_0", "read", json!({"file_path": "a.txt"}))]),
            tool_calls_with_inputs_sse(&[(
                "call_1",
                "apply_patch",
                json!({"input": "*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch"}),
            )]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, data_dir, cwd, id) =
        setup_agent(&server.uri(), ToolSet::builtin_with_patch()).await;
    fs::write(cwd.path().join("a.txt"), "one\n").unwrap();

    agent
        .send(UserMessage::text("先读再打补丁"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(tool_result(&store, "call_0"), (false, None));
    assert_eq!(tool_result(&store, "call_1"), (false, None));
    assert_eq!(
        fs::read_to_string(cwd.path().join("a.txt")).unwrap(),
        "two\n"
    );
}
