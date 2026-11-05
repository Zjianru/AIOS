//! 接缝 1：请求层重试、用量事件、thinking 解析/回放、取消等响应头
//! （测试清单第 9、10、11 条 + T004.3 审计扩的两笔）。

use std::time::Duration;

use crate::common::{
    FINAL_TEXT_SSE, HeadersGatedServer, ScriptedSseServer, SequenceResponder, SseScript,
    setup_agent, sse, wait_idle,
};
use agent::{AgentEvent, Delivery, TurnOutcome};
use model::{ContentBlock, UserMessage};
use session::{EventKind, SessionStore, TurnEndReason};
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

fn http_error(status: u16, error_type: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(serde_json::json!({
        "error": { "type": error_type, "message": "测试错误" }
    }))
}

fn drain_events(events: &mut tokio::sync::broadcast::Receiver<AgentEvent>) -> Vec<AgentEvent> {
    let mut collected = Vec::new();
    while let Ok(event) = events.try_recv() {
        collected.push(event);
    }
    collected
}

fn retries(events: &[AgentEvent]) -> Vec<(u32, u32, u64, String)> {
    events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::Retrying {
                attempt,
                max_attempts,
                delay_ms,
                code,
            } => Some((*attempt, *max_attempts, *delay_ms, code.clone())),
            _ => None,
        })
        .collect()
}

fn assistant_blocks(store: &session::SessionSnapshot) -> Vec<Vec<ContentBlock>> {
    store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantMessage { message, .. } => Some(message.content.clone()),
            _ => None,
        })
        .collect()
}

const THINKING_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_think","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"先想"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"一下"}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig-abc"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"答案"}}

event: content_block_stop
data: {"type":"content_block_stop","index":1}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

const PARTIAL_USAGE_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_usage","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"好的"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":7}}

event: message_stop
data: {"type":"message_stop"}

"#;

const MIDSTREAM_ERROR_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_mid","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":5,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"半截"}}

event: error
data: {"type":"error","error":{"type":"rate_limit_error","message":"流中途限流"}}

"#;

#[tokio::test]
async fn rate_limit_retries_then_succeeds() {
    let server = MockServer::start().await;
    mount_responses(
        &server,
        vec![
            http_error(429, "rate_limit_error"),
            http_error(429, "rate_limit_error"),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("重试一下"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 15).await;

    let collected = drain_events(&mut events);
    let retries = retries(&collected);
    assert_eq!(retries.len(), 2, "429 两次要发 2 次重试事件：{retries:?}");
    assert_eq!(retries[0].0, 1);
    assert_eq!(retries[1].0, 2);
    assert!(
        retries
            .iter()
            .all(|retry| retry.1 == 5 && retry.3 == "RATE_LIMIT")
    );
    assert_eq!(server.received_requests().await.unwrap().len(), 3);
    assert!(
        collected.iter().any(|event| matches!(
            event,
            AgentEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
                ..
            }
        )),
        "重试成功后这一轮 completed"
    );

    // T006.2b：重试写日志（llm/retry + llm/retry-started）。正常一轮 7 个事件 + 2 条 inbox
    // splice（收下、领取）+ 2 次重试 × 2 条 = 13。
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(store.events().len(), 13);
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, EventKind::LlmRetry { .. }))
            .count(),
        2,
        "两次排定的重试都落 llm/retry"
    );
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, EventKind::LlmRetryStarted { .. }))
            .count(),
        2,
        "两次等待完成后都落 llm/retry-started"
    );
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, EventKind::TurnStart { .. }))
            .count(),
        1
    );
}

#[tokio::test]
async fn auth_error_does_not_retry() {
    let server = MockServer::start().await;
    mount_responses(&server, vec![http_error(401, "authentication_error")]).await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("认证失败"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let collected = drain_events(&mut events);
    assert!(retries(&collected).is_empty(), "401 不重试");
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
    assert!(
        collected.iter().any(|event| matches!(
            event,
            AgentEvent::TurnEnded {
                outcome: TurnOutcome::Error { code, .. },
                ..
            } if code == "AUTH"
        )),
        "401 以 AUTH error 结束"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TurnEndReason::Error {
            error: session::LlmFailure {
                code: "AUTH".to_owned(),
                message: "模型请求失败: AUTH: 测试错误".to_owned(),
            },
        }]
    );
}

#[tokio::test]
async fn failure_after_streamed_content_is_retried_like_dsh() {
    // T006.2b 改回 DSH：已流出可见内容的重试没有额外门槛（agent.ts:544-564）。半截内容
    // 只留在第一次 assistant/attempt 的 stream 里，不进消息、不进下一次请求。
    let server = MockServer::start().await;
    mount_responses(
        &server,
        vec![
            sse(MIDSTREAM_ERROR_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ],
    )
    .await;
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("流到一半失败"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let collected = drain_events(&mut events);
    let retries = retries(&collected);
    assert_eq!(retries.len(), 1, "已流出内容也重试（DSH）：{retries:?}");
    assert_eq!(retries[0].3, "RATE_LIMIT");
    assert_eq!(server.received_requests().await.unwrap().len(), 2);
    assert!(
        collected.iter().any(|event| matches!(
            event,
            AgentEvent::TurnEnded {
                outcome: TurnOutcome::Completed,
                ..
            }
        )),
        "重试成功后这一轮 completed"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        assistant_blocks(&store),
        vec![vec![ContentBlock::Text {
            text: "工具都跑完了。".to_owned()
        }]],
        "只有重试成功的那条 assistant 消息"
    );
    assert!(
        store
            .events()
            .iter()
            .any(|event| matches!(event.kind, EventKind::AssistantAttempt { .. })),
        "第一次失败尝试落 assistant/attempt"
    );
}

#[tokio::test]
async fn usage_event_marks_missing_fields_unknown() {
    let server = MockServer::start().await;
    mount_responses(&server, vec![sse(PARTIAL_USAGE_SSE.to_owned())]).await;
    let (agent, _data_dir, _cwd, _id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("报用量"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let collected = drain_events(&mut events);
    let usage = collected
        .iter()
        .find_map(|event| match event {
            AgentEvent::UsageReported { usage } => Some(usage.clone()),
            _ => None,
        })
        .expect("应当收到用量事件");
    assert_eq!(usage.input_tokens, Some(10));
    assert_eq!(usage.output_tokens, Some(7));
    assert_eq!(usage.cache_read_input_tokens, None);
    assert_eq!(usage.cache_creation_input_tokens, None);
    let summary = usage.summary();
    assert!(summary.contains("缓存读 未知"), "{summary}");
    assert!(summary.contains("缓存写 未知"), "{summary}");
}

#[tokio::test]
async fn thinking_round_trips_with_signature() {
    let server = MockServer::start().await;
    mount_responses(
        &server,
        vec![sse(THINKING_SSE.to_owned()), sse(FINAL_TEXT_SSE.to_owned())],
    )
    .await;
    let (agent, data_dir, cwd, id) = setup_agent(&server.uri(), ToolSet::empty()).await;
    let canonical_cwd = std::fs::canonicalize(cwd.path()).unwrap();
    agent
        .send(UserMessage::text("想一想"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let blocks = &assistant_blocks(&store)[0];
    assert_eq!(
        blocks,
        &vec![
            ContentBlock::Reasoning {
                text: "先想一下".to_owned(),
                signature: Some("sig-abc".to_owned()),
            },
            ContentBlock::Text {
                text: "答案".to_owned()
            },
        ],
        "thinking 进历史、signature 存在块上"
    );

    // 下一次请求要按协议把 thinking 回放回去（带 signature）。
    agent
        .send(UserMessage::text("继续"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 10).await;
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let body_text = String::from_utf8(requests[1].body.clone()).unwrap();
    let body_text = body_text.replace(canonical_cwd.to_str().unwrap(), "<CWD>");
    let body: serde_json::Value = serde_json::from_str(&body_text).unwrap();
    let messages = body["messages"].as_array().unwrap();
    let assistant = messages
        .iter()
        .find(|message| message["role"] == "assistant")
        .expect("请求里要有 assistant 历史");
    let content = assistant["content"].as_array().unwrap();
    assert!(
        content.iter().any(|block| block["type"] == "thinking"
            && block["thinking"] == "先想一下"
            && block["signature"] == "sig-abc"),
        "回放要有 thinking 块和 signature：{content:?}"
    );
    insta::assert_json_snapshot!("thinking_replay_second_request", body);
}

#[tokio::test]
async fn cancel_while_waiting_headers_aborts_promptly() {
    let mut server = HeadersGatedServer::start();
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("等响应头"), Delivery::FollowUp)
        .await
        .unwrap();
    // 请求已经收到、响应头还没写：此时取消必须立刻生效。
    server.wait_request().await;
    agent.cancel().await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), agent.idle())
            .await
            .is_ok(),
        "等响应头阶段取消不能把 idle() 挂住"
    );

    let collected = drain_events(&mut events);
    assert!(
        !collected
            .iter()
            .any(|event| matches!(event, AgentEvent::AssistantTextDelta { .. }))
    );
    assert!(collected.iter().any(|event| matches!(
        event,
        AgentEvent::TurnEnded {
            outcome: TurnOutcome::Aborted,
            ..
        }
    )));
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        vec![TurnEndReason::Aborted {
            reason: session::AbortCause::User
        }]
    );
    drop(server);
}

const PARTIAL_THINKING_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_partial","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"想一想"}}

event: content_block_start
data: {"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"可见"}}

"#;

#[tokio::test]
async fn cancel_keeps_interrupted_thinking_without_signature() {
    let mut server = ScriptedSseServer::start(vec![SseScript {
        frames: PARTIAL_THINKING_SSE.to_owned(),
        hold: true,
    }]);
    let (agent, data_dir, _cwd, id) = setup_agent(&server.base_url(), ToolSet::empty()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("边想边说"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    let mut visible = String::new();
    while visible != "可见" {
        match events.recv().await {
            Ok(AgentEvent::AssistantTextDelta { text }) => visible.push_str(&text),
            Ok(_) => {}
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    agent.cancel().await.unwrap();
    server.release();
    wait_idle(&agent, 5).await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let mut interrupted = None;
    for event in store.events() {
        if let EventKind::AssistantMessage {
            message,
            interrupted: flag,
            ..
        } = &event.kind
            && flag == &Some(true)
        {
            interrupted = Some(message.content.clone());
        }
    }
    assert_eq!(
        interrupted.expect("取消要留 interrupted 消息"),
        vec![
            ContentBlock::Reasoning {
                text: "想一想".to_owned(),
                signature: None,
            },
            ContentBlock::Text {
                text: "可见".to_owned()
            },
        ],
        "interrupted 内容要含 thinking 前缀，且不带 signature"
    );
}
