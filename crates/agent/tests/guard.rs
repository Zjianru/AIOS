//! 接缝 1：重复调用提醒与工具级超时（测试清单第 12、13 条 stub 红测试）。

mod common;

use std::time::Duration;

use agent::{Agent, AgentEvent, Delivery, TurnOutcome};
use common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse,
    tool_calls_with_inputs_sse, wait_idle,
};
use model::UserMessage;
use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFuture, ToolResult, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

const GENTLE: &str = "You are repeating the exact same tool call with identical arguments.";

async fn setup(
    server: &MockServer,
    tools: Option<ToolSet>,
) -> (Agent, tempfile::TempDir, session::SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let config = isolated_config(data_dir.path());
    let model_config = model_config(&server.uri());
    let agent = Agent::create(
        config,
        agent::Deps {
            model: model::DeepSeek::new(model_config).unwrap(),
            tools: tools.unwrap_or_else(ToolSet::builtin),
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, id)
}

async fn mount(server: &MockServer, responses: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            responses.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

fn glob_call(pattern: &str) -> String {
    tool_calls_with_inputs_sse(&[("call_1", "glob", json!({"pattern": pattern}))])
}

async fn bodies(server: &MockServer) -> Vec<String> {
    server
        .received_requests()
        .await
        .unwrap()
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .collect()
}

#[tokio::test]
async fn repeated_identical_calls_trigger_gentle_reminder() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            glob_call("*.rs"),
            glob_call("*.rs"),
            glob_call("*.rs"),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 4);
    assert!(!bodies[2].contains(GENTLE), "第三次调用之前不应有提醒");
    assert!(
        bodies[3].contains(GENTLE),
        "第三次连续重复后下一步应带提醒：{}",
        bodies[3]
    );
}

#[tokio::test]
async fn different_arguments_do_not_trigger_reminder() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            glob_call("a*"),
            glob_call("b*"),
            glob_call("c*"),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    for body in bodies(&server).await {
        assert!(!body.contains(GENTLE));
    }
}

#[tokio::test]
async fn single_call_does_not_trigger_reminder() {
    let server = MockServer::start().await;
    mount(&server, vec![glob_call("*.rs"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    for body in bodies(&server).await {
        assert!(!body.contains(GENTLE));
    }
}

#[tokio::test]
async fn reminder_is_not_repeated_within_one_step() {
    let server = MockServer::start().await;
    let five = || {
        tool_calls_with_inputs_sse(&[
            ("call_1", "glob", json!({"pattern": "*.rs"})),
            ("call_2", "glob", json!({"pattern": "*.rs"})),
            ("call_3", "glob", json!({"pattern": "*.rs"})),
            ("call_4", "glob", json!({"pattern": "*.rs"})),
            ("call_5", "glob", json!({"pattern": "*.rs"})),
        ])
    };
    mount(&server, vec![five(), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    let count = bodies[1].matches(GENTLE).count();
    assert_eq!(count, 1, "同一步最多一条提醒：{count}");
}

#[tokio::test]
async fn new_user_message_resets_repeat_chain() {
    let server = MockServer::start().await;
    let two = || {
        tool_calls_with_inputs_sse(&[
            ("call_1", "glob", json!({"pattern": "*.rs"})),
            ("call_2", "glob", json!({"pattern": "*.rs"})),
        ])
    };
    mount(
        &server,
        vec![
            two(),
            FINAL_TEXT_SSE.to_owned(),
            glob_call("*.rs"),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    agent.send(UserMessage::text("new instruction"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    for body in bodies(&server).await {
        assert!(!body.contains(GENTLE), "新用户消息应清零重复计数");
    }
}

struct TimeoutTool;

impl Tool for TimeoutTool {
    fn name(&self) -> &str {
        "slow_timeout"
    }
    fn description(&self) -> &str {
        "test tool declaring a timeout"
    }
    fn parameters(&self) -> &Value {
        static P: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        P.get_or_init(|| json!({"type": "object", "properties": {}}))
    }
    fn timeout_ms(&self) -> Option<u64> {
        Some(10)
    }
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Exclusive
    }
    fn execute<'a>(&'a self, call: ToolCall, _c: &'a ToolContext) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            ToolResult::success(call.id, "slow done")
        })
    }
}

#[tokio::test]
async fn timed_out_tool_returns_error_and_turn_continues() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![
            tool_calls_with_inputs_sse(&[("call_1", "slow_timeout", json!({}))]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (agent, _data, _id) = setup(&server, Some(ToolSet::new(vec![Box::new(TimeoutTool)]))).await;
    let mut events = agent.subscribe();
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "超时后这一轮继续发下一次请求");
    assert!(
        bodies[1].contains("Error: tool call timed out after 10ms"),
        "模型应收到超时错误：{}",
        bodies[1]
    );
    let mut completed = false;
    while let Ok(event) = events.try_recv() {
        if matches!(
            event,
            AgentEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
                ..
            }
        ) {
            completed = true;
        }
    }
    assert!(completed, "超时是工具结果错误，这一轮应正常完成");
}

fn repeated_calls(n: usize, pattern: &str) -> Vec<String> {
    (0..n).map(|_| glob_call(pattern)).collect()
}

fn raw_tool_call_sse(id: &str, name: &str, input_json: &str) -> String {
    let message_start = serde_json::json!({"type": "message_start", "message": {"id": "msg_raw", "type": "message", "role": "assistant", "model": "deepseek-flash", "content": [], "usage": {"input_tokens": 1, "output_tokens": 1}}}).to_string();
    let input: Value = serde_json::from_str(input_json).unwrap();
    let block_start = serde_json::json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": id, "name": name, "input": input}}).to_string();
    let message_delta = serde_json::json!({"type": "message_delta", "delta": {"stop_reason": "tool_use"}, "usage": {"output_tokens": 2}}).to_string();
    let block_stop = serde_json::json!({"type": "content_block_stop", "index": 0}).to_string();
    let message_stop = serde_json::json!({"type": "message_stop"}).to_string();
    format!(
        "event: message_start\ndata: {message_start}\n\nevent: content_block_start\ndata: {block_start}\n\nevent: content_block_stop\ndata: {block_stop}\n\nevent: message_delta\ndata: {message_delta}\n\nevent: message_stop\ndata: {message_stop}\n\n"
    )
}

#[tokio::test]
async fn fifth_and_eighth_repeats_send_detailed_reminders() {
    let server = MockServer::start().await;
    let mut responses = repeated_calls(8, "*.rs");
    responses.push(FINAL_TEXT_SSE.to_owned());
    mount(&server, responses).await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert!(
        bodies[5].contains("Repeated tool call detected:"),
        "第 5 次应触发详细提醒"
    );
    assert!(bodies[5].contains("consecutive_calls: 5"));
    assert!(
        bodies[8].contains("consecutive_calls: 8"),
        "第 8 次应触发详细提醒"
    );
}

#[tokio::test]
async fn detailed_reminder_truncates_arguments_preview_at_500() {
    let server = MockServer::start().await;
    let long = "x".repeat(700);
    let mut responses = repeated_calls(5, &long);
    responses.push(FINAL_TEXT_SSE.to_owned());
    mount(&server, responses).await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    let detailed = &bodies[5];
    assert!(detailed.contains("Repeated tool call detected:"));
    assert!(detailed.contains("…"), "参数预览应被截断加省略号");
}

#[tokio::test]
async fn todo_write_is_not_tracked_by_repeat_guard() {
    let server = MockServer::start().await;
    let call = || {
        tool_calls_with_inputs_sse(&[(
            "call_1",
            "todo_write",
            json!({"todos": [{"content": "x", "status": "pending"}]}),
        )])
    };
    mount(
        &server,
        vec![call(), call(), call(), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    for body in bodies(&server).await {
        assert!(!body.contains(GENTLE), "todo_write 不跟踪");
    }
}

#[tokio::test]
async fn error_results_also_count_towards_repeats() {
    let server = MockServer::start().await;
    let call = || {
        tool_calls_with_inputs_sse(&[(
            "call_1",
            "read",
            json!({"file_path": "/no/such/guard-file.txt"}),
        )])
    };
    mount(
        &server,
        vec![call(), call(), call(), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert!(bodies[3].contains(GENTLE), "错误结果也计数：{}", bodies[3]);
}

#[tokio::test]
async fn canonical_key_ignores_argument_key_order() {
    let server = MockServer::start().await;
    let a = raw_tool_call_sse("c1", "read", "{\"file_path\":\"/no/such/x\",\"offset\":0}");
    let b = raw_tool_call_sse("c2", "read", "{\"offset\":0,\"file_path\":\"/no/such/x\"}");
    mount(&server, vec![a, b.clone(), b, FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, _data, _id) = setup(&server, None).await;
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert!(
        bodies[3].contains(GENTLE),
        "属性顺序不同也算同一调用：{}",
        bodies[3]
    );
}
