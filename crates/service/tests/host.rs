//! T006.16a：载体无关服务宿主的红测试（调用面 / requestId 幂等 / 事件面）。
//!
//! DSH 对照：packages/api/session-controller/src/index.ts 的 prompt/cancel/events/state 方法面；
//! requestId 幂等对应 DSH prompt 的 requestId（:465 起的 prompt 方法，重复受理不重复插入）。

use std::time::Duration;

use agent::{Agent, AgentConfig};
use model::{DeepSeek, DeepSeekConfig, Effort};
use serde_json::json;
use service::{
    AnswerRequest, CancelRequest, CompactRequest, DeliveryMode, EventsRequest, FollowRequest,
    SendRequest, ServiceAnswer, ServiceEvent, ServiceHost, StatusRequest,
};
use session::{SessionId, SessionStore};
use tokio::sync::broadcast;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const FINAL_TEXT_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_final","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":9,"output_tokens":2}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"完成。"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":3}}

event: message_stop
data: {"type":"message_stop"}

"#;

/// 一次 ask_user_question（两个问题：q1 选项、q2 自定义）。
const ASK_TWO_SSE: &str = r#"event: message_start
data: {"type":"message_start","message":{"id":"m_ask","type":"message","role":"assistant","model":"deepseek-flash","content":[],"usage":{"input_tokens":10,"output_tokens":1}}}

event: content_block_start
data: {"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call_ask","name":"ask_user_question","input":{}}}

event: content_block_delta
data: {"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"questions\":[{\"id\":\"q1\",\"question\":\"P1?\",\"options\":[{\"label\":\"Yes\"},{\"label\":\"No\"}]},{\"id\":\"q2\",\"question\":\"P2?\"}]}"}}

event: content_block_stop
data: {"type":"content_block_stop","index":0}

event: message_delta
data: {"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":2}}

event: message_stop
data: {"type":"message_stop"}

"#;

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

async fn mount(server: &MockServer, responses: Vec<ResponseTemplate>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(service_sequence(responses))
        .mount(server)
        .await;
}

/// 顺序回放（wiremock 的 Respond 实现；与 agent 测试的 SequenceResponder 同形）。
fn service_sequence(responses: Vec<ResponseTemplate>) -> Sequence {
    Sequence {
        responses: std::sync::Mutex::new(responses),
    }
}

struct Sequence {
    responses: std::sync::Mutex<Vec<ResponseTemplate>>,
}

impl wiremock::Respond for Sequence {
    fn respond(&self, _request: &wiremock::Request) -> ResponseTemplate {
        let mut responses = self.responses.lock().unwrap();
        if responses.len() > 1 {
            responses.remove(0)
        } else {
            responses
                .first()
                .cloned()
                .unwrap_or_else(|| ResponseTemplate::new(500))
        }
    }
}

async fn setup(server: &MockServer) -> (Agent, tempfile::TempDir, SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let config = AgentConfig {
        instruction_roots: vec![data_dir.path().join("agents-home")],
        skill_roots: vec![data_dir.path().join("skills-home")],
        ..AgentConfig::default()
    };
    let model = DeepSeek::new(DeepSeekConfig {
        base_url: server.uri(),
        api_key: "test-key".to_owned(),
        model: "deepseek-flash".to_owned(),
        max_tokens: 2048,
        context_window: model::DEFAULT_CONTEXT_WINDOW,
        effort: Effort::High,
    })
    .unwrap();
    let agent = Agent::create(
        config,
        agent::Deps {
            model,
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, id)
}

fn send_request(request_id: &str, text: &str) -> SendRequest {
    SendRequest {
        request_id: request_id.to_owned(),
        content: text.to_owned(),
        delivery: DeliveryMode::FollowUp,
    }
}

async fn next_event(events: &mut broadcast::Receiver<ServiceEvent>) -> ServiceEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("服务事件超时")
        .expect("服务事件流不应关闭")
}

/// 收到 turn/end 或超时就返回（不 panic，便于红期做断言）。
async fn wait_for(
    events: &mut broadcast::Receiver<ServiceEvent>,
    event_type: &str,
    count: usize,
) -> Vec<ServiceEvent> {
    let mut seen = Vec::new();
    let mut matching = 0;
    loop {
        let event = next_event(events).await;
        if event.event_type == event_type {
            matching += 1;
        }
        seen.push(event);
        if matching >= count {
            return seen;
        }
    }
}

async fn collect_turn_events(events: &mut broadcast::Receiver<ServiceEvent>) -> Vec<ServiceEvent> {
    let mut collected = Vec::new();
    loop {
        let next = tokio::time::timeout(Duration::from_millis(800), events.recv()).await;
        match next {
            Ok(Ok(event)) => {
                let done = event.event_type == "turn/end";
                collected.push(event);
                if done {
                    break;
                }
            }
            _ => break,
        }
    }
    collected
}

fn turn_ends(data_dir: &std::path::Path, id: &SessionId) -> Vec<session::TurnEndReason> {
    SessionStore::open_read_only(data_dir, id)
        .unwrap()
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn send_with_same_request_id_enters_once() {
    let server = MockServer::start().await;
    mount(&server, vec![sse(FINAL_TEXT_SSE)]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());

    let first = service.send(send_request("req-1", "你好")).await;
    assert!(first.is_ok(), "第一次 send 应成功：{first:?}");
    let first = first.unwrap();
    assert!(first.accepted, "第一次要受理");
    assert!(!first.duplicate, "第一次不是重复");

    let second = service.send(send_request("req-1", "你好")).await;
    assert!(
        second.is_ok(),
        "重复 requestId 也要返回原始受理：{second:?}"
    );
    let second = second.unwrap();
    assert!(second.duplicate, "同 requestId 第二次要标 duplicate");
    assert!(second.accepted, "重复请求返回原始受理（accepted=true）");

    agent.idle().await;
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "同一个 requestId 只能触发一次模型请求"
    );
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let user_messages = store
        .events()
        .iter()
        .filter(|event| matches!(event.kind, session::EventKind::UserMessage { .. }))
        .count();
    assert_eq!(user_messages, 1, "同 requestId 只进一次会话日志");
}

#[tokio::test]
async fn cancel_interrupts_active_turn() {
    let server = MockServer::start().await;
    // 延迟 10s 的响应：send 之后模型请求停在等待里，用于取消。
    mount(
        &server,
        vec![sse(FINAL_TEXT_SSE).set_delay(Duration::from_secs(10))],
    )
    .await;
    let (agent, data_dir, id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());
    let mut events = service.subscribe_events();

    let started = service.send(send_request("req-cancel", "跑一轮")).await;
    assert!(started.is_ok(), "send 应成功：{started:?}");
    wait_for(&mut events, "turn/start", 1).await;

    let cancelled = service.cancel(CancelRequest {}).await;
    assert!(cancelled.is_ok(), "cancel 应成功：{cancelled:?}");
    assert!(cancelled.unwrap().cancelled, "cancel 要报告已请求取消");
    agent.idle().await;
    assert_eq!(
        turn_ends(data_dir.path(), &id),
        vec![session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        }],
        "取消后这一轮以 aborted 收尾"
    );
}

#[tokio::test]
async fn answer_applies_whole_batch_with_selected_and_custom() {
    let server = MockServer::start().await;
    mount(&server, vec![sse(ASK_TWO_SSE), sse(FINAL_TEXT_SSE)]).await;
    let (agent, _data_dir, _id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());
    let mut events = service.subscribe_events();

    let started = service
        .send(send_request("req-answer", "要问两个问题"))
        .await;
    assert!(started.is_ok(), "send 应成功：{started:?}");
    // 等到两个 question/asked 都已落日志（前面还会有 agent/inbox/spliced 等事件）。
    wait_for(&mut events, "question/asked", 2).await;

    let answered = service
        .answer(AnswerRequest {
            answers: vec![
                ServiceAnswer {
                    question_id: "q1".to_owned(),
                    selected: vec!["Yes".to_owned()],
                    custom: None,
                },
                ServiceAnswer {
                    question_id: "q2".to_owned(),
                    selected: Vec::new(),
                    custom: Some("我自己的答案".to_owned()),
                },
            ],
        })
        .await;
    assert!(answered.is_ok(), "整批回答应成功：{answered:?}");
    assert_eq!(answered.unwrap().applied, 2, "整批两条都要被接受");
    agent.idle().await;

    let requests = server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "回答后续上同一轮");
    let second = String::from_utf8(requests[1].body.clone()).unwrap();
    let payload = tool_result_payload(&second, "call_ask");
    assert_eq!(
        payload,
        json!({"answers": [
            {"id": "q1", "selected": ["Yes"]},
            {"id": "q2", "selected": [], "custom": "我自己的答案"}
        ]}),
        "tool/result 要带 T006.13 的 selected/custom 批形状"
    );
}

/// 从请求体里取某个 tool_use_id 的 tool_result 内容（T006.13 形状）。
fn tool_result_payload(body: &str, call_id: &str) -> serde_json::Value {
    let request: serde_json::Value = serde_json::from_str(body).expect("请求体是 JSON");
    for message in request["messages"].as_array().expect("messages") {
        let Some(content) = message["content"].as_array() else {
            continue;
        };
        for block in content {
            if block["type"] == "tool_result" && block["tool_use_id"] == call_id {
                let text = block["content"][0]["text"]
                    .as_str()
                    .expect("tool_result text");
                return serde_json::from_str(text).expect("tool_result 内容是 JSON");
            }
        }
    }
    panic!("找不到 {call_id} 的 tool_result");
}

#[tokio::test]
async fn compact_returns_first_version_shape() {
    let server = MockServer::start().await;
    mount(&server, vec![sse(FINAL_TEXT_SSE)]).await;
    let (agent, _data_dir, _id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());
    // 没有任何可压缩历史：方法要通、结果形状是 Option（None）。
    let response = service.compact(CompactRequest::default()).await;
    assert!(response.is_ok(), "compact 应成功：{response:?}");
    assert!(
        response.unwrap().compacted.is_none(),
        "没有可压缩历史时 compacted=None"
    );
}

#[tokio::test]
async fn status_reports_object_phase_and_pending_questions() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![sse(FINAL_TEXT_SSE).set_delay(Duration::from_secs(10))],
    )
    .await;
    let (agent, _data_dir, id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());

    let idle = service.status(StatusRequest {}).await;
    assert!(idle.is_ok(), "status 应成功：{idle:?}");
    let idle = idle.unwrap();
    assert_eq!(idle.object.session_id, id.as_str());
    assert!(!idle.running, "空闲时 running=false");
    assert_eq!(idle.pending_questions, 0);

    let mut events = service.subscribe_events();
    assert!(service.send(send_request("req-status", "跑")).await.is_ok());
    wait_for(&mut events, "turn/start", 1).await;
    let running = service.status(StatusRequest {}).await;
    assert!(running.is_ok(), "运行中 status 也要成功：{running:?}");
    assert!(running.unwrap().running, "运行中 running=true");
    assert!(service.cancel(CancelRequest {}).await.is_ok());
    agent.idle().await;
}

#[tokio::test]
async fn events_carry_contiguous_seq_type_and_data() {
    let server = MockServer::start().await;
    mount(&server, vec![sse(FINAL_TEXT_SSE)]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());
    let mut events = service.subscribe_events();

    assert!(
        service
            .send(send_request("req-events", "事件面"))
            .await
            .is_ok()
    );
    agent.idle().await;
    let observed = collect_turn_events(&mut events).await;
    assert!(!observed.is_empty(), "服务事件面要能收到日志事件");

    let types: Vec<&str> = observed
        .iter()
        .map(|event| event.event_type.as_str())
        .collect();
    for expected in ["user/message", "assistant/message", "turn/end"] {
        assert!(types.contains(&expected), "事件面缺 {expected}：{types:?}");
    }
    assert_eq!(observed[0].seq, 1, "服务事件从日志 seq=1 开始");
    let turn_start = observed
        .iter()
        .find(|event| event.event_type == "turn/start")
        .expect("事件面要有 turn/start");
    assert_eq!(
        turn_start.data["turn"], 1,
        "data 是事件字段（不含 type）：{turn_start:?}"
    );
    for pair in observed.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq + 1, "服务事件 seq 必须连续");
    }

    // 与日志事实逐条对齐（seq/type）。
    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let logged: Vec<(u64, &str)> = store
        .events()
        .iter()
        .map(|event| (event.seq, event.kind.type_name()))
        .collect();
    let seen: Vec<(u64, &str)> = observed
        .iter()
        .map(|event| (event.seq, event.event_type.as_str()))
        .collect();
    assert_eq!(seen, logged, "服务事件必须与日志事件一致");
}

#[tokio::test]
async fn follow_fills_from_last_seq_contiguously() {
    let server = MockServer::start().await;
    mount(&server, vec![sse(FINAL_TEXT_SSE), sse(FINAL_TEXT_SSE)]).await;
    let (agent, _data_dir, _id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());

    // 两轮事件。
    assert!(service.send(send_request("req-f1", "一")).await.is_ok());
    agent.idle().await;
    assert!(service.send(send_request("req-f2", "二")).await.is_ok());
    agent.idle().await;
    let all = agent.session_events_from(0);
    assert!(all.len() > 2);

    // 从头补：全部事件、seq 连续、最后序号一致。
    let opening = service.follow(FollowRequest { last_seq: 0 }).await;
    assert!(opening.is_ok(), "follow 应成功：{opening:?}");
    let opening = opening.unwrap();
    assert_eq!(opening.last_seq, agent.last_seq());
    let seqs: Vec<u64> = opening.events.iter().map(|event| event.seq).collect();
    assert_eq!(
        seqs,
        (1..=opening.last_seq).collect::<Vec<_>>(),
        "follow 要连续补齐"
    );
    assert_eq!(opening.events.len(), all.len());

    // 从中途补：只给 lastSeq 之后的事件，不重复。
    let middle = all.len() / 2;
    let middle_seq = all[middle].seq;
    let tail = service
        .follow(FollowRequest {
            last_seq: middle_seq,
        })
        .await
        .unwrap();
    assert_eq!(
        tail.events.first().map(|event| event.seq),
        Some(middle_seq + 1),
        "补齐要从 lastSeq 的下一条开始"
    );
    assert_eq!(tail.events.len(), all.len() - middle - 1);

    // 已是最新：空补齐、序号保持。
    let none = service
        .follow(FollowRequest {
            last_seq: opening.last_seq,
        })
        .await
        .unwrap();
    assert!(none.events.is_empty(), "已经跟到最后没有新事件");
    assert_eq!(none.last_seq, opening.last_seq, "空补齐保持最后序号");
}

#[tokio::test]
async fn events_page_is_turn_aligned_and_budgeted() {
    let server = MockServer::start().await;
    mount(&server, vec![sse(FINAL_TEXT_SSE), sse(FINAL_TEXT_SSE)]).await;
    let (agent, _data_dir, _id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());

    for (index, request_id) in ["req-e1", "req-e2"].iter().enumerate() {
        assert!(service.send(send_request(request_id, "页")).await.is_ok());
        agent.idle().await;
        let _ = index;
    }
    let all = agent.session_events_from(0);
    let turn_starts: Vec<u64> = all
        .iter()
        .filter(|event| event.kind.type_name() == "turn/start")
        .map(|event| event.seq)
        .collect();
    assert_eq!(turn_starts.len(), 2, "两轮数据");

    // 默认预算（50/2/500）在只有两轮时：日志耗尽，整段返回、没有更多。
    let whole = service.events(EventsRequest::default()).await;
    assert!(whole.is_ok(), "events 应成功：{whole:?}");
    let whole = whole.unwrap();
    assert_eq!(whole.events.len(), all.len(), "日志耗尽时返回整段");
    assert!(!whole.has_more, "没有更早的页");
    assert_eq!(whole.events[0].seq, 1, "日志耗尽时页首就是日志开头");

    // 收紧轮预算（minMessages=1/minTurns=1）：页首对齐到最近一轮的 turn/start。
    let budget = EventsRequest {
        min_messages: Some(1),
        min_turns: Some(1),
        ..EventsRequest::default()
    };
    let last_turn = service.events(budget.clone()).await;
    assert!(last_turn.is_ok(), "收紧预算的 events 应成功：{last_turn:?}");
    let last_turn = last_turn.unwrap();
    assert!(last_turn.has_more, "只翻最近一轮时还有更早的");
    assert_eq!(
        last_turn.events[0].event_type, "turn/start",
        "页首要落在轮起点"
    );
    assert_eq!(last_turn.events[0].seq, turn_starts[1]);

    // 用 beforeSeq 往回翻：与上一页首尾相接、不重叠。
    let earlier = service
        .events(EventsRequest {
            before_seq: Some(last_turn.events[0].seq - 1),
            min_messages: Some(1),
            min_turns: Some(1),
            ..EventsRequest::default()
        })
        .await
        .unwrap();
    assert_eq!(
        earlier.events[0].event_type, "turn/start",
        "下一页的页首也是轮起点"
    );
    assert_eq!(
        earlier.events.last().map(|event| event.seq),
        Some(last_turn.events[0].seq - 1),
        "两页首尾相接"
    );
    // 页首之前只剩发送时的 inbox 事件（seq 1），不算额外一轮；hasMore 如实反映。
    assert_eq!(earlier.has_more, earlier.events[0].seq > all[0].seq);

    // maxMessages=1：预算先到，页可能不在轮起点（DSH：预算或耗尽赢过轮对齐）。
    let capped = service
        .events(EventsRequest {
            max_messages: Some(1),
            min_messages: Some(1),
            min_turns: Some(1),
            ..EventsRequest::default()
        })
        .await
        .unwrap();
    assert!(capped.has_more);
    assert_eq!(
        capped
            .events
            .iter()
            .filter(|event| {
                matches!(
                    event.event_type.as_str(),
                    "user/message" | "assistant/message"
                )
            })
            .count(),
        1,
        "maxMessages=1 只允许一条消息"
    );
}

#[tokio::test]
async fn shutdown_cancels_active_turn_and_keeps_log_resumable() {
    let server = MockServer::start().await;
    // 延迟响应：让轮子停在等模型上，用于验证停服务取消。
    mount(
        &server,
        vec![sse(FINAL_TEXT_SSE).set_delay(Duration::from_secs(10))],
    )
    .await;
    let (agent, data_dir, id) = setup(&server).await;
    let service = ServiceHost::new(agent.clone());
    let mut events = service.subscribe_events();

    assert!(
        service
            .send(send_request("req-shutdown", "停服务"))
            .await
            .is_ok()
    );
    wait_for(&mut events, "turn/start", 1).await;
    service.shutdown().await;
    agent.idle().await;
    assert_eq!(
        turn_ends(data_dir.path(), &id),
        vec![session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        }],
        "停服务时正在跑的一轮要按取消收尾"
    );

    drop(service);
    drop(agent);
    // 日志完整、锁释放：下次能续接。
    let store = SessionStore::open(data_dir.path(), &id).expect("停服务后应能续接（写锁已释放）");
    assert!(
        store
            .events()
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::TurnEnd { .. })),
        "日志里要有 turn/end 收尾"
    );
}
