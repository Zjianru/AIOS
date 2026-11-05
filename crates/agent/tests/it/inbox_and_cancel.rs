//! 接缝 1：收件箱（steer / inject / follow-up）、取消与失唤醒回归（测试清单第 4、5、6 条 + 票面加项）。



use agent::{Agent, AgentConfig, AgentEvent, Delivery, Deps, TurnOutcome};
use crate::common::{
    FINAL_TEXT_SSE, PARTIAL_TEXT_SSE, ScriptedSseServer, SequenceResponder, SseScript, ToolEvent,
    gated, model_config, sse, tool_calls_sse,
};
use model::{ContentBlock, DeepSeek, UserMessage};
use session::{EventKind, SessionStore, TurnEndReason};
use tokio::sync::mpsc;
use tools::{ToolConcurrency, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn new_agent(
    base_url: &str,
    tools: ToolSet,
    max_parallel_tool_calls: usize,
) -> (
    Agent,
    tempfile::TempDir,
    tempfile::TempDir,
    session::SessionId,
) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let session_id = store.id().clone();
    let agent = Agent::create(
        AgentConfig {
            max_parallel_tool_calls,
            ..crate::common::isolated_config(data_dir.path())
        },
        Deps {
            model: DeepSeek::with_credentials(model_config(base_url), crate::common::test_credential())
                .unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, cwd, session_id)
}

async fn mount_sequence(server: &MockServer, responses: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            responses.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

fn count_turns(events: &[session::Event]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event.kind, EventKind::TurnStart { .. }))
        .count()
}

fn turn_reasons(events: &[session::Event]) -> Vec<TurnEndReason> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

fn assistant_text_of(events: &[session::Event], interrupted: bool) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::AssistantMessage {
                message,
                interrupted: flag,
                ..
            } if flag.unwrap_or(false) == interrupted => Some(
                message
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .collect::<String>(),
            ),
            _ => None,
        })
        .collect()
}

fn user_texts(body: &serde_json::Value) -> Vec<String> {
    let mut texts = Vec::new();
    if let Some(messages) = body["messages"].as_array() {
        for message in messages {
            if let Some(content) = message["content"].as_array() {
                for block in content {
                    if block["type"] == "text"
                        && let Some(text) = block["text"].as_str()
                    {
                        texts.push(text.to_owned());
                    }
                }
            }
        }
    }
    texts
}

fn parse_body(request: &wiremock::Request) -> serde_json::Value {
    serde_json::from_slice(&request.body).expect("请求体是 JSON")
}

#[tokio::test]
async fn steer_enters_next_step_while_running() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            tool_calls_sse(&[("call_0", "gate")]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), tools, 10).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("开始"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned())),
        "工具先跑起来，这一轮还没结束"
    );
    // 模型这一轮还在跑，插一句 steer：它要进下一步的请求。
    agent
        .send(UserMessage::text("插一句"), Delivery::Steer)
        .await
        .unwrap();
    permits[0].1.add_permits(1);
    agent.idle().await;

    let mut streamed = String::new();
    let mut outcome = None;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::AssistantTextDelta { text } => streamed.push_str(&text),
            AgentEvent::TurnEnded { outcome: ended, .. } => outcome = Some(ended),
            _ => {}
        }
    }
    assert_eq!(outcome, Some(TurnOutcome::Completed));
    assert!(streamed.contains("工具都跑完了。"));

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let kinds: Vec<&str> = reopened
        .events()
        .iter()
        .map(|event| event.kind.type_name())
        .collect();
    assert_eq!(
        kinds,
        vec![
            "agent/inbox/spliced",
            "turn/start",
            "agent/inbox/spliced",
            "step/start",
            "system/message",
            "user/message",
            "assistant/message",
            "tool/call",
            "agent/inbox/spliced",
            "tool/result",
            "step/end",
            "agent/inbox/spliced",
            "step/start",
            "user/message",
            "assistant/message",
            "step/end",
            "turn/end"
        ],
        "steer 在第二个 step 的 user/message 进入请求（T006.2a：inbox splice 按发生位置落日志）"
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let second = parse_body(&requests[1]);
    assert!(
        user_texts(&second).contains(&"插一句".to_owned()),
        "第二个请求里要带 steer 的那句话：{second}"
    );
}

#[tokio::test]
async fn inject_enters_next_step_without_waking_idle_loop() {
    let server = MockServer::start().await;
    mount_sequence(&server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), ToolSet::empty(), 10).await;
    // 空闲时 inject：下一步的请求要带上它，但不唤醒 loop、不产生 turn。
    agent
        .send(UserMessage::text("注入"), Delivery::Inject)
        .await
        .unwrap();
    agent.idle().await;
    let after_inject = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        after_inject.events().len(),
        1,
        "inject 不唤醒空闲的 loop：只落一条 durable 的 inbox 事件，不开 turn"
    );
    assert_eq!(
        after_inject.events()[0].kind.type_name(),
        "agent/inbox/spliced"
    );
    drop(after_inject);

    agent
        .send(UserMessage::text("继续"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(count_turns(reopened.events()), 1);
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body = parse_body(&requests[0]);
    let texts = user_texts(&body);
    let inject_position = texts.iter().position(|text| text == "注入");
    let follow_position = texts.iter().position(|text| text == "继续");
    assert!(
        inject_position.is_some() && follow_position.is_some(),
        "inject 的消息要进下一步的请求：{texts:?}"
    );
    assert!(
        inject_position < follow_position,
        "inject 在 follow-up 之前"
    );
}

#[tokio::test]
async fn follow_up_opens_next_turn() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            tool_calls_sse(&[("call_0", "gate")]),
            FINAL_TEXT_SSE.to_owned(),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), tools, 10).await;
    agent
        .send(UserMessage::text("第一句"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    agent
        .send(UserMessage::text("下一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    permits[0].1.add_permits(1);
    agent.idle().await;

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        count_turns(reopened.events()),
        2,
        "follow-up 在这一轮结束后开下一轮"
    );
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Completed, TurnEndReason::Completed]
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 3);
    let last = parse_body(&requests[2]);
    assert!(user_texts(&last).contains(&"下一轮".to_owned()));
}

#[tokio::test]
async fn cancel_mid_stream_keeps_interrupted_text() {
    let mut server = ScriptedSseServer::start(vec![
        SseScript {
            frames: PARTIAL_TEXT_SSE.to_owned(),
            hold: true,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let (agent, data_dir, _cwd, session_id) =
        new_agent(&server.base_url(), ToolSet::empty(), 10).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("开始流"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    // 先确认订阅者真的收到了已流出的文本（driver 已处理完这部分帧，而服务端
    // 还 hold 着连接），再取消；否则取消可能赶在第一个帧被读到之前。
    let mut streamed = String::new();
    while streamed != "已经流出来的部分文字。" {
        match events.recv().await {
            Ok(AgentEvent::AssistantTextDelta { text }) => streamed.push_str(&text),
            Ok(_) => {}
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    agent.cancel().await.unwrap();
    agent.idle().await;
    let mut outcome = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome: ended, .. } = event {
            outcome = Some(ended);
        }
    }
    assert_eq!(streamed, "已经流出来的部分文字。");
    assert_eq!(outcome, Some(TurnOutcome::Aborted));

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let interrupted = assistant_text_of(reopened.events(), true);
    assert_eq!(
        interrupted,
        vec!["已经流出来的部分文字。".to_owned()],
        "取消时按 interruptedBlocks 规则保留已流出的文本前缀"
    );
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Aborted {
            reason: session::AbortCause::User
        }]
    );

    // 取消之后会话还能接着用。
    server.release();
    agent
        .send(UserMessage::text("再来"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(count_turns(reopened.events()), 2);
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![
            TurnEndReason::Aborted {
                reason: session::AbortCause::User
            },
            TurnEndReason::Completed
        ]
    );
}

#[tokio::test]
async fn cancel_batch_skips_unstarted_calls() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            tool_calls_sse(&[("call_0", "gate"), ("call_1", "gate2")]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![
        gated("gate", ToolConcurrency::Parallel, &started_tx, &mut permits),
        gated(
            "gate2",
            ToolConcurrency::Parallel,
            &started_tx,
            &mut permits,
        ),
    ]);
    // 上限 1：gate 跑起来时 gate2 还在排队，取消时它应当被跳过。
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), tools, 1).await;
    agent
        .send(UserMessage::text("跑工具"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    agent.cancel().await.unwrap();
    permits[0].1.add_permits(1);
    agent.idle().await;

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let results: Vec<(String, String, Option<String>)> = reopened
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolResult { message, error, .. } => {
                let tool_use_id = message.tool_call_id.clone()?;
                let content: String = message
                    .content
                    .iter()
                    .filter_map(ContentBlock::as_text)
                    .collect();
                Some((
                    tool_use_id,
                    content,
                    error.as_ref().map(|error| error.code.clone()),
                ))
            }
            _ => None,
        })
        .collect();
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].0, "call_0");
    assert!(
        !results[0].1.starts_with("Error:"),
        "已开始的调用要保留结果"
    );
    assert_eq!(results[1].0, "call_1");
    assert_eq!(results[1].1, "Error: tool call aborted before dispatch");
    assert_eq!(results[1].2.as_deref(), Some("ABORTED_BEFORE_DISPATCH"));
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Aborted {
            reason: session::AbortCause::User
        }]
    );

    // 取消之后会话还能接着用。
    agent
        .send(UserMessage::text("继续"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(count_turns(reopened.events()), 2);
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![
            TurnEndReason::Aborted {
                reason: session::AbortCause::User
            },
            TurnEndReason::Completed
        ]
    );
}

async fn lost_wake_case(delivery: Delivery) {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![FINAL_TEXT_SSE.to_owned(), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), ToolSet::empty(), 10).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    // driver 在收尾检查之后会显式让出一次调度；这里收到 TurnEnded 就投递，
    // 正好落在「收尾检查 → settle 快照」的窗口里（不靠 sleep）。
    loop {
        match events.recv().await {
            Ok(AgentEvent::TurnEnded { .. }) => break,
            Ok(_) => continue,
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    agent
        .send(UserMessage::text("窗口消息"), delivery)
        .await
        .unwrap();
    agent.idle().await;

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        count_turns(reopened.events()),
        2,
        "窗口里投递的消息不能滞留：应当再开一轮"
    );
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Completed, TurnEndReason::Completed]
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let last = parse_body(&requests[1]);
    assert!(
        user_texts(&last).contains(&"窗口消息".to_owned()),
        "第二个请求要带窗口里投递的消息"
    );
}

#[tokio::test]
async fn lost_wake_window_keeps_follow_up() {
    lost_wake_case(Delivery::FollowUp).await;
}

#[tokio::test]
async fn lost_wake_window_keeps_steer() {
    lost_wake_case(Delivery::Steer).await;
}

/// 造一段「空白关闭块 + 非空关闭块 + 开着的 text 块 + 开着的 tool_use 块」的 SSE。
fn interrupted_blocks_sse() -> String {
    let mut body = String::from(
        "event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"id\":\"msg_blocks\",\"type\":\"message\",\"role\":\"assistant\",\"model\":\"deepseek-flash\",\"content\":[],\"usage\":{\"input_tokens\":10,\"output_tokens\":1}}}\n\n",
    );
    // 关闭的空白块：取消时丢弃。
    body.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n");
    body.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"\\n  \"}}\n\n");
    body.push_str(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":0}\n\n",
    );
    // 关闭的非空块：保留。
    body.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":1,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n");
    body.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":1,\"delta\":{\"type\":\"text_delta\",\"text\":\"保留的旧块\"}}\n\n");
    body.push_str(
        "event: content_block_stop\ndata: {\"type\":\"content_block_stop\",\"index\":1}\n\n",
    );
    // 还开着的 text 块：用已收到的增量保留。
    body.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":2,\"content_block\":{\"type\":\"text\",\"text\":\"\"}}\n\n");
    body.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":2,\"delta\":{\"type\":\"text_delta\",\"text\":\"开着的块\"}}\n\n");
    // 还开着的 tool_use 块：取消时整个丢弃。
    body.push_str("event: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":3,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call_x\",\"name\":\"read\",\"input\":{}}}\n\n");
    body.push_str("event: content_block_delta\ndata: {\"type\":\"content_block_delta\",\"index\":3,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{}\"}}\n\n");
    body
}

#[tokio::test]
async fn cancel_mid_stream_then_send_makes_exactly_one_more_turn() {
    let mut server = ScriptedSseServer::start(vec![
        SseScript {
            frames: PARTIAL_TEXT_SSE.to_owned(),
            hold: true,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let (agent, data_dir, _cwd, session_id) =
        new_agent(&server.base_url(), ToolSet::empty(), 10).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("开始"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    let mut streamed = String::new();
    while streamed != "已经流出来的部分文字。" {
        match events.recv().await {
            Ok(AgentEvent::AssistantTextDelta { text }) => streamed.push_str(&text),
            Ok(_) => {}
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    // 取消后、这一轮收尾前立刻投递：修复前这里会让 agent 用同一个已 abort 的
    // Phase 空转（idle 不返回、日志无界增长）。
    agent.cancel().await.unwrap();
    agent
        .send(UserMessage::text("取消后再来一句"), Delivery::FollowUp)
        .await
        .unwrap();
    server.release();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), agent.idle())
            .await
            .is_ok(),
        "取消后立刻投递时 idle() 必须返回（不能空转）"
    );

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        count_turns(reopened.events()),
        2,
        "只允许 aborted + completed 两轮，不能有空的轮次"
    );
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![
            TurnEndReason::Aborted {
                reason: session::AbortCause::User
            },
            TurnEndReason::Completed
        ]
    );
    assert_eq!(
        assistant_text_of(reopened.events(), true),
        vec!["已经流出来的部分文字。".to_owned()],
        "第一轮的 interrupted 消息保留"
    );
    let bodies = server.request_bodies();
    assert_eq!(bodies.len(), 2);
    let body: serde_json::Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert!(
        user_texts(&body).contains(&"取消后再来一句".to_owned()),
        "第二轮请求要带取消后投递的消息"
    );
}

#[tokio::test]
async fn cancel_during_tool_then_send_makes_exactly_one_more_turn() {
    let server = MockServer::start().await;
    mount_sequence(
        &server,
        vec![
            tool_calls_sse(&[("call_0", "gate")]),
            FINAL_TEXT_SSE.to_owned(),
        ],
    )
    .await;
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), tools, 10).await;
    agent
        .send(UserMessage::text("跑工具"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    // 取消要等已开始的工具结束，这个窗口里立刻投递 steer。
    agent.cancel().await.unwrap();
    agent
        .send(UserMessage::text("工具窗口里插话"), Delivery::Steer)
        .await
        .unwrap();
    permits[0].1.add_permits(1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), agent.idle())
            .await
            .is_ok(),
        "取消后立刻投递时 idle() 必须返回（不能空转）"
    );

    let mut starts = 1;
    while let Ok(event) = started.try_recv() {
        if let ToolEvent::Started(_) = event {
            starts += 1;
        }
    }
    assert_eq!(starts, 1, "工具不能被重跑");

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        count_turns(reopened.events()),
        2,
        "只允许 aborted + completed 两轮，不能有空的轮次"
    );
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![
            TurnEndReason::Aborted {
                reason: session::AbortCause::User
            },
            TurnEndReason::Completed
        ]
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2);
    let body = parse_body(&requests[1]);
    assert!(
        user_texts(&body).contains(&"工具窗口里插话".to_owned()),
        "第二轮请求要带取消后投递的 steer"
    );
}

#[tokio::test]
async fn cancel_keeps_only_safe_interrupted_blocks() {
    let mut server = ScriptedSseServer::start(vec![SseScript {
        frames: interrupted_blocks_sse(),
        hold: true,
    }]);
    let (agent, data_dir, _cwd, session_id) =
        new_agent(&server.base_url(), ToolSet::empty(), 10).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("开始"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    let mut streamed = String::new();
    while !streamed.ends_with("开着的块") {
        match events.recv().await {
            Ok(AgentEvent::AssistantTextDelta { text }) => streamed.push_str(&text),
            Ok(_) => {}
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    agent.cancel().await.unwrap();
    server.release();
    agent.idle().await;

    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    let mut interrupted = None;
    for event in reopened.events() {
        if let EventKind::AssistantMessage {
            message,
            interrupted: flag,
            ..
        } = &event.kind
            && flag == &Some(true)
        {
            interrupted = Some(message.clone());
        }
    }
    let message = interrupted.expect("取消时应有 interrupted 消息");
    assert_eq!(
        message
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect::<Vec<_>>(),
        vec!["保留的旧块", "开着的块"],
        "空白块丢弃、关闭块保留、打开块用增量保留"
    );
    assert_eq!(
        message.content.len(),
        2,
        "tool-call 块不能进 interrupted 消息"
    );
    assert!(
        message
            .content
            .iter()
            .all(|block| matches!(block, ContentBlock::Text { .. }))
    );
}

#[tokio::test]
async fn steer_wakes_idle_loop() {
    let server = MockServer::start().await;
    mount_sequence(&server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, _cwd, session_id) = new_agent(&server.uri(), ToolSet::empty(), 10).await;
    // 空闲时投递 steer：要唤醒 loop、开一轮，并进入下一步的请求。
    agent
        .send(UserMessage::text("插一句"), Delivery::Steer)
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(3), agent.idle())
            .await
            .is_ok(),
        "steer 要唤醒空闲的 loop"
    );
    let reopened = SessionStore::open_read_only(data_dir.path(), &session_id).unwrap();
    assert_eq!(count_turns(reopened.events()), 1);
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Completed]
    );
    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    assert!(user_texts(&parse_body(&requests[0])).contains(&"插一句".to_owned()));
}
