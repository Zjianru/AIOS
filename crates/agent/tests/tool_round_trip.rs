//! 接缝 1：工具调用 → 第二次请求带着工具结果（测试清单第 2 条）。

mod common;

use agent::{AgentEvent, Delivery, TurnOutcome};
use common::{SequenceResponder, setup_agent, sse};
use model::{ContentBlock, Role, UserMessage};
use session::{EventKind, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const FIRST_RESPONSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":20,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"我先读一下文件。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"call_1","name":"read","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"file_path\":\"note.txt\"}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

const SECOND_RESPONSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_2","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":40,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"文件里写着 hello。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn tool_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(FIRST_RESPONSE.to_owned()),
            sse(SECOND_RESPONSE.to_owned()),
        ]))
        .mount(&server)
        .await;

    let (agent, data_dir, cwd, session_id) = setup_agent(&server.uri(), ToolSet::builtin()).await;
    std::fs::write(cwd.path().join("note.txt"), "hello\n").unwrap();
    let canonical_cwd = std::fs::canonicalize(cwd.path()).unwrap();
    let mut events = agent.subscribe();
    agent.send(UserMessage::text("请读一下 note.txt"), Delivery::FollowUp);
    agent.idle().await;

    let mut text = String::new();
    let mut outcome = None;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::AssistantTextDelta { text: delta } => text.push_str(&delta),
            AgentEvent::TurnEnded { outcome: ended, .. } => outcome = Some(ended),
            _ => {}
        }
    }
    assert_eq!(text, "我先读一下文件。文件里写着 hello。");
    assert_eq!(outcome, Some(TurnOutcome::Completed));
    drop(agent);

    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
    let kinds: Vec<&str> = reopened
        .events()
        .iter()
        .map(|event| event.kind.type_name())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "turn/start",
            "step/start",
            "system/message",
            "user/message",
            "assistant/message",
            "tool/call",
            "tool/result",
            "step/end",
            "step/start",
            "assistant/message",
            "step/end",
            "turn/end"
        ],
        "工具调用一轮的事件顺序"
    );

    // 第一次 assistant 消息里有文本块和工具调用块
    let assistant_tool_call = reopened
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::AssistantMessage { message, .. }
                if message
                    .content
                    .iter()
                    .any(|block| block.as_tool_call().is_some()) =>
            {
                Some(message)
            }
            _ => None,
        })
        .expect("日志里有带工具调用的 assistant/message");
    assert_eq!(
        assistant_tool_call.content,
        vec![
            ContentBlock::text("我先读一下文件。"),
            ContentBlock::ToolCall {
                id: "call_1".to_owned(),
                name: "read".to_owned(),
                arguments: "{\"file_path\":\"note.txt\"}".to_owned(),
            }
        ]
    );

    // tool/result 里是 read 工具的真实结果
    let tool_result = reopened
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::ToolResult {
                message,
                error_code,
                ..
            } => Some((message, error_code)),
            _ => None,
        })
        .expect("日志里有 tool/result");
    assert_eq!(tool_result.0.role, Role::Tool);
    assert_eq!(tool_result.1, &None);
    let ContentBlock::ToolResult {
        tool_use_id,
        content,
        is_error,
    } = &tool_result.0.content[0]
    else {
        panic!("tool/result 消息里应当是 ToolResult 块");
    };
    assert_eq!(tool_use_id, "call_1");
    assert!(!is_error);
    assert!(content.contains("<path>"));
    assert!(content.contains("1: hello"));
    assert!(content.contains("(End of file - total 1 lines)"));

    // 第二个请求：assistant 的 tool_use + user 的 tool_result
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "一轮工具调用发两次请求");
    let body_text = String::from_utf8(requests[1].body.clone()).unwrap();
    let canonical = canonical_cwd.to_str().unwrap();
    let body_text = body_text.replace(canonical, "<CWD>");
    let body: serde_json::Value = serde_json::from_str(&body_text).unwrap();
    let messages = body["messages"].as_array().expect("messages 是数组");
    assert_eq!(
        messages.len(),
        3,
        "第二个请求带 user、assistant、tool result 三条消息"
    );
    assert_eq!(messages[1]["content"][1]["type"], "tool_use");
    assert_eq!(messages[1]["content"][1]["id"], "call_1");
    assert_eq!(messages[1]["content"][1]["input"]["file_path"], "note.txt");
    assert_eq!(messages[2]["role"], "user");
    assert_eq!(messages[2]["content"][0]["type"], "tool_result");
    assert_eq!(messages[2]["content"][0]["tool_use_id"], "call_1");
    assert!(
        messages[2]["content"][0]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("1: hello")
    );
    insta::assert_json_snapshot!("tool_round_trip_second_request", body);
}
