//! 接缝 1：写类工具经由 Agent 驱动（测试清单第 1、2 条 + D5 加项）。

mod common;

use std::fs;

use agent::Delivery;
use common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse, tool_calls_with_inputs_sse};
use model::{ContentBlock, UserMessage};
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

/// 会话日志里某个 call_id 的 tool/result：(is_error, error_code)。
fn tool_result(store: &SessionStore, call_id: &str) -> (bool, Option<String>) {
    for event in store.events() {
        let EventKind::ToolResult {
            message,
            error_code,
            ..
        } = &event.kind
        else {
            continue;
        };
        let Some(ContentBlock::ToolResult {
            tool_use_id,
            is_error,
            ..
        }) = message.content.first()
        else {
            continue;
        };
        if tool_use_id == call_id {
            return (*is_error, error_code.clone());
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
    let (agent, data_dir, cwd, id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    fs::write(cwd.path().join("a.txt"), "old\n").unwrap();

    agent.send(UserMessage::text("先读再写"), Delivery::FollowUp);
    agent.idle().await;

    let store = SessionStore::open(data_dir.path(), &id).unwrap();
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

    agent.send(UserMessage::text("写再读"), Delivery::FollowUp);
    agent.idle().await;

    let store = SessionStore::open(data_dir.path(), &id).unwrap();
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
