//! T006.2b：重试日志（llm/retry + llm/retry-started）、已流出内容的重试规则、usage 缺失不伪造。
//!
//! DSH 依据：packages/llm/llm-retry/src/index.ts:140-186（durable before wait、started after wait）、
//! packages/llm/llm-retry/src/types.ts:9-40（事件数据形状）、
//! packages/core/agent-loop/src/agent.ts:544-564（已流出内容的重试没有额外门槛）。

mod common;

use std::fs;
use std::path::Path;

use agent::{AgentEvent, Delivery};
use common::{FINAL_TEXT_SSE, SequenceResponder, setup_agent, sse, wait_idle};
use model::UserMessage;
use serde_json::{Value, json};
use session::{SessionId, SessionStore, TurnEndReason};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

async fn mount_responses(server: &MockServer, responses: Vec<ResponseTemplate>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(responses))
        .mount(server)
        .await;
}

/// 带 retry-after 的 HTTP 错误；retry-after 用小数秒（测试里压到毫秒级，避免真等 500ms 起步）。
fn http_error_after(status: u16, error_type: &str, retry_after: &str) -> ResponseTemplate {
    ResponseTemplate::new(status)
        .insert_header("retry-after", retry_after)
        .set_body_json(json!({
            "error": { "type": error_type, "message": "测试错误" }
        }))
}

fn log_rows(data_dir: &Path, id: &SessionId) -> Vec<Value> {
    let path = data_dir.join("sessions").join(format!("{id}.jsonl"));
    let text = fs::read_to_string(path).unwrap();
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn events_of<'a>(rows: &'a [Value], event_type: &str) -> Vec<&'a Value> {
    rows.iter()
        .filter(|row| row["type"] == event_type)
        .map(|row| &row["data"])
        .collect()
}

fn position(rows: &[Value], event_type: &str) -> usize {
    rows.iter()
        .position(|row| row["type"] == event_type)
        .unwrap_or_else(|| panic!("日志里没有 {event_type}"))
}

/// 只有 message_start + message_stop 的空响应：适配器归 EMPTY_RESPONSE（可重试）。
const EMPTY_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_empty","type":"message","role":"assistant","model":"deepseek-flash","content":[]}}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

event: message_stop
data: {"type":"message_stop"}

"#;

/// 流出半截文本后报可重试的限流错误（DSH 不看可见内容、照样重试）。
const PARTIAL_THEN_RATE_LIMIT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_mid","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":5,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"半截"}}

event: error
data: {"type":"error","error":{"type":"rate_limit_error","message":"流中途限流"}}

"#;

/// 完全没有 usage 字段的回答：K10 要求不显示、不伪造。
const NO_USAGE_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_nousage","type":"message","role":"assistant","model":"deepseek-flash","content":[]}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"没有用量"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"}}

event: message_stop
data: {"type":"message_stop"}

"#;

#[tokio::test]
async fn retry_events_record_scheduled_and_started() {
    let server = MockServer::start().await;
    mount_responses(
        &server,
        vec![sse(EMPTY_SSE.to_owned()), sse(FINAL_TEXT_SSE.to_owned())],
    )
    .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("空响应后重试"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    // 订阅者事件照旧（AgentEvent::Retrying）。
    let mut retrying = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::Retrying {
            attempt,
            max_attempts,
            delay_ms,
            code,
        } = event
        {
            retrying.push((attempt, max_attempts, delay_ms, code));
        }
    }
    assert_eq!(retrying.len(), 1, "一次重试一条 Retrying");
    assert_eq!((retrying[0].0, retrying[0].1), (1, 5));
    assert_eq!(retrying[0].3, "EMPTY_RESPONSE");
    assert!(retrying[0].2 >= 1, "delayMs 是正数：{retrying:?}");

    let rows = log_rows(data_dir.path(), &id);
    let retries = events_of(&rows, "llm/retry");
    let started = events_of(&rows, "llm/retry-started");
    assert_eq!(retries.len(), 1, "每次排定的重试都落 llm/retry");
    assert_eq!(started.len(), 1, "等待结束后落 llm/retry-started");
    let retry = retries[0];
    let retry_id = retry["retryId"].as_str().expect("retryId 是字符串");
    assert!(!retry_id.is_empty());
    assert_eq!(retry["turn"], 1);
    assert_eq!(retry["step"], 1);
    assert_eq!(retry["provider"], "deepseek");
    assert_eq!(retry["mode"], "normal");
    assert!(
        retry["policyKey"]
            .as_str()
            .is_some_and(|key| key.contains("normal") && key.contains("EMPTY_RESPONSE")),
        "policyKey 记录固定策略：{retry}"
    );
    assert_eq!(retry["retry"], 1);
    assert_eq!(retry["maxRetries"], 5);
    assert!(retry["delayMs"].as_u64().is_some_and(|delay| delay >= 1));
    assert_eq!(retry["failure"]["code"], "EMPTY_RESPONSE");
    assert!(
        retry["failure"]["message"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "failure 带可读消息：{retry}"
    );

    let started = started[0];
    assert_eq!(started["retryId"], retry_id, "同一条重试链共用一个 retryId");
    assert_eq!(started["turn"], 1);
    assert_eq!(started["step"], 1);
    assert_eq!(started["retry"], 1);

    // 时点照 DSH：attempt → llm/retry → llm/retry-started → 下一次尝试的 message。
    assert!(position(&rows, "assistant/attempt") < position(&rows, "llm/retry"));
    assert!(position(&rows, "llm/retry") < position(&rows, "llm/retry-started"));
    assert!(position(&rows, "llm/retry-started") < position(&rows, "assistant/message"));
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
}

#[tokio::test]
async fn max_retries_exhausted_after_five_retries() {
    let server = MockServer::start().await;
    let responses = (0..6)
        .map(|_| http_error_after(500, "api_error", "0.005"))
        .collect();
    mount_responses(&server, responses).await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("一直 500"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let rows = log_rows(data_dir.path(), &id);
    let retries = events_of(&rows, "llm/retry");
    let started = events_of(&rows, "llm/retry-started");
    assert_eq!(retries.len(), 5, "最多 5 次重试：{retries:?}");
    assert_eq!(started.len(), 5);
    assert_eq!(
        retries
            .iter()
            .map(|retry| retry["retry"].as_u64().unwrap())
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4, 5],
        "retry 从 1 连续编号"
    );
    let retry_ids: Vec<&str> = retries
        .iter()
        .map(|retry| retry["retryId"].as_str().unwrap())
        .collect();
    assert!(
        retry_ids.windows(2).all(|pair| pair[0] == pair[1]),
        "同一条重试链共用一个 retryId：{retry_ids:?}"
    );
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        6,
        "首发 + 5 次重试"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TurnEndReason::Error {
            error: session::LlmFailure {
                message: "模型请求失败: SERVER: 测试错误".to_owned(),
                code: "SERVER".to_owned(),
            },
        }],
        "重试耗尽按最后的错误码收尾"
    );
}

#[tokio::test]
async fn retryable_codes_are_retried_by_code() {
    // K10 点名：EMPTY_RESPONSE / RATE_LIMIT / SERVER 三个可重试码都要真的按码重试。
    let server = MockServer::start().await;
    mount_responses(
        &server,
        vec![
            sse(EMPTY_SSE.to_owned()),
            http_error_after(429, "rate_limit_error", "0.005"),
            http_error_after(500, "api_error", "0.005"),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("三种错误码"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let rows = log_rows(data_dir.path(), &id);
    let retries = events_of(&rows, "llm/retry");
    assert_eq!(
        retries
            .iter()
            .map(|retry| retry["failure"]["code"].as_str().unwrap())
            .collect::<Vec<_>>(),
        vec!["EMPTY_RESPONSE", "RATE_LIMIT", "SERVER"],
        "按码记录失败身份"
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 4);
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TurnEndReason::Completed]
    );
}

#[tokio::test]
async fn retry_after_streamed_content_matches_dsh() {
    // DSH agent.ts:544-564：流里已经流出文本也照样重试——半截内容只在 attempt 的 stream 里，
    // 不进消息、不进下一次请求。
    let server = MockServer::start().await;
    mount_responses(
        &server,
        vec![
            sse(PARTIAL_THEN_RATE_LIMIT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    agent
        .send(UserMessage::text("流到一半失败"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let rows = log_rows(data_dir.path(), &id);
    assert_eq!(events_of(&rows, "llm/retry").len(), 1, "有可见内容也重试");
    let attempts = events_of(&rows, "assistant/attempt");
    assert_eq!(attempts.len(), 1);
    let attempt_stream = attempts[0]["stream"].as_array().unwrap();
    assert!(
        attempt_stream
            .iter()
            .any(|record| record["type"] == "text-chunks"
                && record["texts"]
                    .as_array()
                    .is_some_and(|texts| texts.iter().any(|text| text == "半截"))),
        "半截文本留在 attempt 的 stream 里：{attempt_stream:?}"
    );
    let messages = events_of(&rows, "assistant/message");
    assert_eq!(messages.len(), 1, "只有重试成功的那条消息");
    assert!(
        messages[0]["message"]["content"][0]["text"] == "工具都跑完了。",
        "最终消息只含成功尝试的内容：{messages:?}"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    assert!(
        !second.contains("半截"),
        "半截文本不进派生历史（下一次请求）：{second}"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TurnEndReason::Completed]
    );
}

#[tokio::test]
async fn usage_absent_is_not_fabricated() {
    let server = MockServer::start().await;
    mount_responses(&server, vec![sse(NO_USAGE_SSE.to_owned())]).await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("没有用量"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let mut usage_events = 0;
    while let Ok(event) = events.try_recv() {
        if matches!(event, AgentEvent::UsageReported { .. }) {
            usage_events += 1;
        }
    }
    assert_eq!(usage_events, 0, "整段 usage 缺失时不能发用量事件");

    let rows = log_rows(data_dir.path(), &id);
    let message = events_of(&rows, "assistant/message")[0];
    assert!(
        message.get("usage").is_none(),
        "usage 缺失时日志里不能伪造空对象：{message}"
    );
    assert_eq!(message["message"]["content"][0]["text"], "没有用量");
}
