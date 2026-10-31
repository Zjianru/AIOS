//! 接缝 1：通过 Agent 的 6 个入口驱动一轮纯文本对话（测试清单第 1 条）。

use agent::{Agent, AgentEvent, Delivery, Deps, TurnOutcome};
use model::{DeepSeek, DeepSeekConfig, Effort, Role, UserMessage};
use session::{EventKind, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SSE_SCRIPT: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"msg_1","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":12,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"，世界"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":5}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn pure_text_turn() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(SSE_SCRIPT),
        )
        .mount(&server)
        .await;

    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let canonical_cwd = std::fs::canonicalize(cwd.path()).unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let session_id = store.id().clone();
    let config = DeepSeekConfig {
        base_url: server.uri(),
        api_key: "test-key".to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        effort: Effort::High,
    };
    let agent = Agent::create(Deps {
        model: DeepSeek::new(config).unwrap(),
        tools: ToolSet::empty(),
        store,
    })
    .await
    .unwrap();
    let mut events = agent.subscribe();
    agent.send(UserMessage::text("你好"), Delivery::FollowUp);
    agent.idle().await;

    let mut streamed = String::new();
    let mut outcome = None;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::AssistantTextDelta { text } => streamed.push_str(&text),
            AgentEvent::TurnEnded { outcome: ended, .. } => outcome = Some(ended),
        }
    }
    assert_eq!(streamed, "你好，世界", "流式文本按增量送到订阅者");
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
            "step/end",
            "turn/end"
        ],
        "一轮纯文本的事件顺序"
    );

    let assistant = reopened
        .events()
        .iter()
        .find_map(|event| match &event.kind {
            EventKind::AssistantMessage { message, usage, .. } => Some((message, usage)),
            _ => None,
        })
        .expect("日志里有 assistant/message");
    let assistant_text: String = assistant
        .0
        .content
        .iter()
        .filter_map(|block| block.as_text())
        .collect();
    assert_eq!(assistant_text, "你好，世界");
    let usage = assistant.1.as_ref().expect("assistant/message 带用量");
    assert_eq!(usage.input_tokens, Some(12));
    assert_eq!(usage.output_tokens, Some(5));

    let derived = reopened.derive_messages();
    let roles: Vec<Role> = derived.iter().map(|message| message.role).collect();
    assert_eq!(
        roles,
        vec![Role::System, Role::User, Role::Assistant],
        "从日志派生出的消息顺序"
    );

    // 派生出的请求体：截下第一个请求做 insta 快照（system 里的临时目录替换成占位符，
    // 让快照稳定）。
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "一轮纯文本只发一次请求");
    let mut body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    let cwd_text = canonical_cwd.to_str().unwrap();
    if let Some(system) = body.get("system").and_then(serde_json::Value::as_str) {
        body["system"] = serde_json::Value::String(system.replace(cwd_text, "<CWD>"));
    }
    insta::assert_json_snapshot!("request_body", body);
}
