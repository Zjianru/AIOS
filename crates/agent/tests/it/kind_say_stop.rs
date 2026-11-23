//! T078.1：成功的 kind_say 这一轮直接结束，不再调模型。

use agent::{Agent, Delivery, Deps};
use model::UserMessage;
use serde_json::Value;
use session::SessionStore;
use tools::{Tool, ToolCall, ToolContext, ToolFuture, ToolResult};

use crate::common::{isolated_config, model_config, test_credential};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SSE_SAY: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":2}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_say","name":"kind_say","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"parts\":[\"第一段\",\"第二段\"]}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

struct SayTool;

impl Tool for SayTool {
    fn name(&self) -> &str {
        "kind_say"
    }

    fn description(&self) -> &str {
        "say parts"
    }

    fn parameters(&self) -> &Value {
        static PARAMS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMS.get_or_init(|| serde_json::json!({"type":"object"}))
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { ToolResult::success(call.id, "Said 2 parts.") })
    }
}

#[tokio::test]
async fn successful_kind_say_ends_the_turn_without_another_model_call() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(SSE_SAY),
        )
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    drop(store);
    let agent = Agent::create(
        isolated_config(data.path()),
        Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                test_credential(),
            )
            .unwrap(),
            tools: tools::ToolSet::new(vec![Box::new(SayTool)]),
            store: SessionStore::open(data.path(), &id).unwrap(),
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("说两段"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "成功 kind_say 后不能再调模型");
    let events = agent.session_events_from(0);
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        session::EventKind::ToolCall { name, .. } if name == "kind_say"
    )));
    assert!(events.iter().any(|event| matches!(
        &event.kind,
        session::EventKind::ToolResult { message, .. } if !message.content.is_empty()
    )));
    assert!(
        events
            .iter()
            .any(|event| matches!(&event.kind, session::EventKind::TurnEnd { .. })),
        "这一轮要结束"
    );
}
