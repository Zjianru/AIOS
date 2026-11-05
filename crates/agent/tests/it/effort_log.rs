//! T012.3 测试清单 9：assistant/message 日志同时记 effort（实际）与 effort_wanted（想要）。

use crate::common::setup_agent_with_model;
use model::UserMessage;
use serde_json::Value;

const SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"MiniMax-M3.1-Flash","content":[],"usage":{"input_tokens":12,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":1}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn assistant_efforts(data_dir: &std::path::Path) -> Option<(String, String)> {
    let dir = data_dir.join("sessions");
    let mut lines = Vec::new();
    for entry in std::fs::read_dir(dir).expect("sessions") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|value| value.to_str()) == Some("jsonl") {
            lines.extend(
                std::fs::read_to_string(path)
                    .expect("读会话日志")
                    .lines()
                    .map(str::to_owned),
            );
        }
    }
    for line in lines {
        let event: Value = serde_json::from_str(&line).expect("JSON");
        if event["type"] == "assistant/message" {
            let effort = event["data"]["effort"].as_str()?.to_owned();
            let wanted = event["data"]["effort_wanted"].as_str()?.to_owned();
            return Some((effort, wanted));
        }
    }
    None
}

#[tokio::test]
async fn m31_flash_logs_actual_low_for_wanted_none() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(SSE),
        )
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, _id) = setup_agent_with_model(
        &server.uri(),
        "MiniMax-M3.1-Flash",
        model::Effort::new("none"),
    )
    .await;
    agent
        .send(UserMessage::text("你好"), agent::Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    drop(agent);
    let (actual, wanted) = assistant_efforts(data_dir.path()).expect("assistant/message 日志");
    assert_eq!(actual, "low", "关不掉思考的模型 wanted none 实际发 low");
    assert_eq!(wanted, "none", "日志保留用户要的档");
}
