//! 接缝 1：收件箱（steer / inject / follow-up）、取消与失唤醒回归（测试清单第 4、5、6 条 + 票面加项）。

mod common;

use agent::{Agent, AgentConfig, AgentEvent, Delivery, Deps, TurnOutcome};
use common::{
    FINAL_TEXT_SSE, PARTIAL_TEXT_SSE, ScriptedSseServer, SequenceResponder, SseScript, ToolEvent,
    gated, sse, tool_calls_sse,
};
use model::{ContentBlock, DeepSeek, DeepSeekConfig, Effort, UserMessage};
use session::{EventKind, SessionStore, TurnEndReason};
use tokio::sync::mpsc;
use tools::{ToolConcurrency, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn model_config(base_url: &str) -> DeepSeekConfig {
    DeepSeekConfig {
        base_url: base_url.to_owned(),
        api_key: "test-key".to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        effort: Effort::High,
    }
}

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
        },
        Deps {
            model: DeepSeek::new(model_config(base_url)).unwrap(),
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
    agent.send(UserMessage::text("开始"), Delivery::FollowUp);
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned())),
        "工具先跑起来，这一轮还没结束"
    );
    // 模型这一轮还在跑，插一句 steer：它要进下一步的请求。
    agent.send(UserMessage::text("插一句"), Delivery::Steer);
    permits[0].1.add_permits(1);
    agent.idle().await;

    let mut streamed = String::new();
    let mut outcome = None;
    while let Ok(event) = events.try_recv() {
        match event {
            AgentEvent::AssistantTextDelta { text } => streamed.push_str(&text),
            AgentEvent::TurnEnded { outcome: ended, .. } => outcome = Some(ended),
        }
    }
    assert_eq!(outcome, Some(TurnOutcome::Completed));
    assert!(streamed.contains("工具都跑完了。"));

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
            "user/message",
            "assistant/message",
            "step/end",
            "turn/end"
        ],
        "steer 在第二个 step 的 user/message 进入请求"
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
    agent.send(UserMessage::text("注入"), Delivery::Inject);
    agent.idle().await;
    let after_inject = SessionStore::open(data_dir.path(), &session_id).unwrap();
    assert_eq!(
        after_inject.events().len(),
        0,
        "inject 不唤醒空闲的 loop，不应产生任何日志事件"
    );
    drop(after_inject);

    agent.send(UserMessage::text("继续"), Delivery::FollowUp);
    agent.idle().await;
    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
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
    agent.send(UserMessage::text("第一句"), Delivery::FollowUp);
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    agent.send(UserMessage::text("下一轮"), Delivery::FollowUp);
    permits[0].1.add_permits(1);
    agent.idle().await;

    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
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
    agent.send(UserMessage::text("开始流"), Delivery::FollowUp);
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
    agent.cancel();
    agent.idle().await;
    let mut outcome = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome: ended, .. } = event {
            outcome = Some(ended);
        }
    }
    assert_eq!(streamed, "已经流出来的部分文字。");
    assert_eq!(outcome, Some(TurnOutcome::Aborted));

    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
    let interrupted = assistant_text_of(reopened.events(), true);
    assert_eq!(
        interrupted,
        vec!["已经流出来的部分文字。".to_owned()],
        "取消时按 interruptedBlocks 规则保留已流出的文本前缀"
    );
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Aborted]
    );

    // 取消之后会话还能接着用。
    server.release();
    agent.send(UserMessage::text("再来"), Delivery::FollowUp);
    agent.idle().await;
    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
    assert_eq!(count_turns(reopened.events()), 2);
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Aborted, TurnEndReason::Completed]
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
    agent.send(UserMessage::text("跑工具"), Delivery::FollowUp);
    assert_eq!(
        started.recv().await,
        Some(ToolEvent::Started("gate".to_owned()))
    );
    agent.cancel();
    permits[0].1.add_permits(1);
    agent.idle().await;

    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
    let results: Vec<(String, String, Option<String>)> = reopened
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::ToolResult {
                message,
                error_code,
                ..
            } => {
                let block = message.content.first()?;
                match block {
                    ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } => Some((tool_use_id.clone(), content.clone(), error_code.clone())),
                    _ => None,
                }
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
        vec![TurnEndReason::Aborted]
    );

    // 取消之后会话还能接着用。
    agent.send(UserMessage::text("继续"), Delivery::FollowUp);
    agent.idle().await;
    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
    assert_eq!(count_turns(reopened.events()), 2);
    assert_eq!(
        turn_reasons(reopened.events()),
        vec![TurnEndReason::Aborted, TurnEndReason::Completed]
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
    agent.send(UserMessage::text("第一轮"), Delivery::FollowUp);
    // driver 在收尾检查之后会显式让出一次调度；这里收到 TurnEnded 就投递，
    // 正好落在「收尾检查 → settle 快照」的窗口里（不靠 sleep）。
    loop {
        match events.recv().await {
            Ok(AgentEvent::TurnEnded { .. }) => break,
            Ok(_) => continue,
            Err(error) => panic!("事件通道关闭: {error}"),
        }
    }
    agent.send(UserMessage::text("窗口消息"), delivery);
    agent.idle().await;

    let reopened = SessionStore::open(data_dir.path(), &session_id).unwrap();
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
