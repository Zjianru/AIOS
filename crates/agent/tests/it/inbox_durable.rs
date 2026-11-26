//! T006.2a：输入队列持久化（agent/inbox/spliced）——重启后已收下的消息恰好进一次。
//!
//! DSH 依据：packages/core/agent-loop/src/inbox.ts:20-64（投影折叠与校验）、:96-122（clear/claim）、
//! :158-231（splice/通知）；README.zh.md:71（每次 inbox 变更提交一条规范化事件；
//! claimed/discarded/inserted 是 dispatch 通知、不落日志）。

use std::fs;
use std::path::{Path, PathBuf};

use crate::common::{
    FINAL_TEXT_SSE, PARTIAL_TEXT_SSE, ScriptedSseServer, SequenceResponder, SseScript,
    isolated_config, model_config, sse, wait_idle,
};
use agent::{Agent, AgentError, AgentEvent, Delivery, Deps};
use model::{DeepSeek, Message, Role, UserMessage};
use serde_json::Value;
use session::{EventKind, InboxOutcome, InboxTarget, SessionId, SessionSnapshot, SessionStore};
use tools::ToolSet;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn log_path(data_dir: &Path, id: &SessionId) -> PathBuf {
    data_dir.join("sessions").join(format!("{id}.jsonl"))
}

/// 日志里的 header 和每个事件行（JSON）。
fn log_rows(data_dir: &Path, id: &SessionId) -> Vec<Value> {
    let text = fs::read_to_string(log_path(data_dir, id)).unwrap();
    text.lines()
        .skip(1)
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn read_snapshot(data_dir: &Path, id: &SessionId) -> SessionSnapshot {
    SessionStore::open_read_only(data_dir, id).unwrap()
}

async fn mount_responses(server: &MockServer, bodies: Vec<String>) {
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(
            bodies.into_iter().map(sse).collect(),
        ))
        .mount(server)
        .await;
}

async fn create_agent(data_dir: &Path, cwd: &Path, base_url: &str) -> (Agent, SessionId) {
    let store = SessionStore::create(data_dir, cwd).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(
                model_config(base_url),
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: ToolSet::empty(),
            store,
        },
    )
    .await
    .unwrap();
    (agent, id)
}

async fn resume_agent(
    data_dir: &Path,
    id: &SessionId,
    base_url: &str,
) -> Result<Agent, AgentError> {
    let store = SessionStore::open(data_dir, id).unwrap();
    Agent::resume(
        id.clone(),
        isolated_config(data_dir),
        Deps {
            model: DeepSeek::with_credentials(
                model_config(base_url),
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: ToolSet::empty(),
            store,
        },
    )
    .await
}

fn message_text(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(model::ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join("")
}

/// 请求体里的 user 文本（按消息顺序）。
fn request_user_texts(body: &Value) -> Vec<String> {
    let Some(messages) = body["messages"].as_array() else {
        return Vec::new();
    };
    messages
        .iter()
        .filter(|message| message["role"] == "user")
        .flat_map(|message| {
            message["content"]
                .as_array()
                .map(|blocks| -> Vec<String> {
                    blocks
                        .iter()
                        .filter(|block| block["type"] == "text")
                        .filter_map(|block| block["text"].as_str().map(str::to_owned))
                        .collect()
                })
                .unwrap_or_default()
        })
        .collect()
}

/// 日志里 user/message 的文本（按顺序）。
fn logged_user_texts(snapshot: &SessionSnapshot) -> Vec<String> {
    snapshot
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::UserMessage { message, .. } => Some(message_text(message)),
            _ => None,
        })
        .collect()
}

fn count_text(texts: &[String], needle: &str) -> usize {
    texts.iter().filter(|text| text.as_str() == needle).count()
}

/// 日志里的一条 `agent/inbox/spliced`。
#[derive(Debug, Clone, PartialEq)]
struct LoggedSplice {
    target: InboxTarget,
    start: u64,
    removed_count: Option<u64>,
    inserted: Vec<Message>,
    outcome: Option<InboxOutcome>,
}

fn splices(snapshot: &SessionSnapshot) -> Vec<LoggedSplice> {
    snapshot
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::InboxSpliced {
                target,
                start,
                removed_count,
                inserted,
                outcome,
            } => Some(LoggedSplice {
                target: *target,
                start: *start,
                removed_count: *removed_count,
                inserted: inserted.clone(),
                outcome: *outcome,
            }),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn inject_send_is_durable_and_notifies_inserted() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri()).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("后台通知"), Delivery::Inject)
        .await
        .unwrap();

    // dispatch 通知 inserted 照 DSH（inbox.ts:227-229），不落日志。
    let mut inserted = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::InboxInserted { message } = event {
            inserted.push(message);
        }
    }
    // 消息 id 是新 uuid（不进日志的 dispatch 通知也带 id），按内容断言；id 用下一条 splice 校验。
    assert_eq!(inserted.len(), 1);
    assert_eq!(message_text(&inserted[0]), "后台通知");
    assert_eq!(inserted[0].role, Role::User);
    assert!(
        server.received_requests().await.unwrap().is_empty(),
        "inject 不唤醒空闲的 loop"
    );

    let snapshot = read_snapshot(data_dir.path(), &id);
    assert_eq!(
        snapshot.events().len(),
        1,
        "inject 只落一条 inbox 事件，不开 turn"
    );
    let EventKind::InboxSpliced {
        target,
        start,
        removed_count,
        inserted,
        outcome,
    } = &snapshot.events()[0].kind
    else {
        panic!("唯一的事件应是 agent/inbox/spliced");
    };
    assert_eq!(*target, InboxTarget::NextStep);
    assert_eq!(*start, 0);
    assert_eq!(*removed_count, None);
    assert_eq!(inserted.len(), 1);
    assert_eq!(message_text(&inserted[0]), "后台通知");
    assert_eq!(
        inserted[0].source,
        model::MessageSource::User { rpc_id: None }
    );
    assert_eq!(*outcome, None);

    // wire 形状照 DSH：data 里 target/start/inserted；可选字段省略。
    let rows = log_rows(data_dir.path(), &id);
    assert_eq!(rows[0]["type"], "agent/inbox/spliced");
    assert_eq!(rows[0]["data"]["target"], "next-step");
    assert_eq!(rows[0]["data"]["start"], 0);
    assert_eq!(rows[0]["data"]["inserted"][0]["role"], "user");
    assert!(rows[0]["data"].get("removedCount").is_none());
    assert!(rows[0]["data"].get("outcome").is_none());
}

#[tokio::test]
async fn pending_inject_survives_resume_and_is_claimed_once() {
    let server1 = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server1.uri()).await;
    agent
        .send(UserMessage::text("后台通知"), Delivery::Inject)
        .await
        .unwrap();
    drop(agent);

    let server2 = MockServer::start().await;
    mount_responses(&server2, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let resumed = resume_agent(data_dir.path(), &id, &server2.uri())
        .await
        .unwrap();
    // DSH 语义：resume 只恢复待处理输入，不自动开 turn（resume.spec.ts:1014）。
    resumed.idle().await;
    assert!(
        server2.received_requests().await.unwrap().is_empty(),
        "resume 不自动开 turn"
    );
    let after_resume = read_snapshot(data_dir.path(), &id).events().len();

    let mut events = resumed.subscribe();
    resumed
        .send(UserMessage::text("继续"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 3).await;

    // claimed 通知（DSH inbox.ts:108-113）：恢复的 inject 与新消息都进第一个 step。
    let mut claimed = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::InboxClaimed { message, turn } = event {
            claimed.push((message_text(&message), turn));
        }
    }
    assert_eq!(
        claimed,
        vec![("后台通知".to_owned(), 1), ("继续".to_owned(), 1)]
    );

    let requests = server2.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1);
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        request_user_texts(&body),
        vec!["后台通知".to_owned(), "继续".to_owned()],
        "恢复的 inject 与新的 follow-up 都进第一个 step，顺序照 claim"
    );

    let after_turn = read_snapshot(data_dir.path(), &id);
    assert_eq!(
        after_turn.events().len(),
        after_resume + 11,
        "一轮事件：insert、turn/start、两条 claim、step/start、system/message、user/message×2、assistant/message、step/end、turn/end"
    );
    let texts = logged_user_texts(&after_turn);
    assert_eq!(count_text(&texts, "后台通知"), 1);
    assert_eq!(count_text(&texts, "继续"), 1);
    let claims: Vec<_> = splices(&after_turn)
        .into_iter()
        .filter(|splice| splice.removed_count.is_some() && splice.outcome.is_none())
        .collect();
    assert_eq!(
        claims.len(),
        2,
        "next-step 与 next-turn 各一条领取 splice：{claims:?}"
    );
    drop(resumed);

    // 平衡日志再重启：历史里的消息不会被再消费一次。
    let server3 = MockServer::start().await;
    mount_responses(&server3, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let again = resume_agent(data_dir.path(), &id, &server3.uri())
        .await
        .unwrap();
    again
        .send(UserMessage::text("再一句"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&again, 3).await;
    let final_texts = logged_user_texts(&read_snapshot(data_dir.path(), &id));
    assert_eq!(count_text(&final_texts, "后台通知"), 1, "重启后不重复消费");
}

#[tokio::test]
async fn crash_window_message_is_claimed_once_after_restart() {
    // 第一轮正常结束，拿到平衡日志。
    let server1 = MockServer::start().await;
    mount_responses(&server1, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server1.uri()).await;
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 3).await;
    drop(agent);

    // 崩溃窗口：消息已收下（spliced 已落盘）、这一轮还没开。
    {
        let mut store = SessionStore::open(data_dir.path(), &id).unwrap();
        store
            .append(&[EventKind::InboxSpliced {
                target: InboxTarget::NextTurn,
                start: 0,
                removed_count: None,
                inserted: vec![Message::text(Role::User, "崩溃前收下的")],
                outcome: None,
            }])
            .unwrap();
    }

    // 重启：旧消息一轮，续接后发的新消息下一轮；两轮回答。
    let server2 = MockServer::start().await;
    mount_responses(
        &server2,
        vec![FINAL_TEXT_SSE.to_owned(), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let resumed = resume_agent(data_dir.path(), &id, &server2.uri())
        .await
        .unwrap();
    assert!(
        server2.received_requests().await.unwrap().is_empty(),
        "resume 不自动开 turn"
    );
    resumed
        .send(UserMessage::text("续接后一句"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 5).await;

    let requests = server2.received_requests().await.unwrap();
    assert_eq!(requests.len(), 2, "旧消息一轮、新消息一轮");
    let first: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert_eq!(
        request_user_texts(&first),
        vec!["第一轮".to_owned(), "崩溃前收下的".to_owned()],
        "第一个请求 = 历史的一轮 + 刚从队列领取的那条"
    );
    let second: Value = serde_json::from_slice(&requests[1].body).unwrap();
    let second_texts = request_user_texts(&second);
    assert_eq!(count_text(&second_texts, "崩溃前收下的"), 1);
    assert_eq!(count_text(&second_texts, "续接后一句"), 1);

    // 恰好一次：日志里这条消息的 user/message 只有一条。
    let texts = logged_user_texts(&read_snapshot(data_dir.path(), &id));
    assert_eq!(count_text(&texts, "崩溃前收下的"), 1, "恰好进一次");
    drop(resumed);

    // 平衡后再重启也不重放。
    let server3 = MockServer::start().await;
    mount_responses(&server3, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let again = resume_agent(data_dir.path(), &id, &server3.uri())
        .await
        .unwrap();
    again.idle().await;
    assert!(
        server3.received_requests().await.unwrap().is_empty(),
        "平衡日志重启不自动开 turn"
    );
    let final_texts = logged_user_texts(&read_snapshot(data_dir.path(), &id));
    assert_eq!(count_text(&final_texts, "崩溃前收下的"), 1);
}

#[tokio::test]
async fn cancel_clears_pending_input_durably() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri()).await;
    agent
        .send(UserMessage::text("要取消的"), Delivery::Inject)
        .await
        .unwrap();
    let mut events = agent.subscribe();
    agent.cancel().await.unwrap();

    // discarded 通知照 DSH（inbox.ts:224-226），不落日志。
    let mut discarded = Vec::new();
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::InboxDiscarded { message } = event {
            discarded.push(message_text(&message));
        }
    }
    assert_eq!(discarded, vec!["要取消的".to_owned()]);

    let snapshot = read_snapshot(data_dir.path(), &id);
    let splices = splices(&snapshot);
    assert_eq!(splices.len(), 2, "插入 + 取消各一条 splice：{splices:?}");
    assert_eq!(splices[0].target, InboxTarget::NextStep);
    assert_eq!(splices[0].inserted.len(), 1);
    assert_eq!(message_text(&splices[0].inserted[0]), "要取消的");
    let canceled = &splices[1];
    assert_eq!(canceled.target, InboxTarget::NextStep);
    assert_eq!(canceled.start, 0);
    assert_eq!(canceled.removed_count, Some(1));
    assert!(canceled.inserted.is_empty());
    assert_eq!(canceled.outcome, Some(InboxOutcome::Canceled));
    // wire：普通删除带 removedCount + outcome: canceled（inbox.ts:214-221）。
    let rows = log_rows(data_dir.path(), &id);
    assert_eq!(rows[1]["data"]["removedCount"], 1);
    assert_eq!(rows[1]["data"]["outcome"], "canceled");
    drop(snapshot);

    // 重启后队列为空：新消息的请求里没有已取消的那条。
    drop(agent); // 单写者锁：模拟重启前先释放旧写者
    let server2 = MockServer::start().await;
    mount_responses(&server2, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let resumed = resume_agent(data_dir.path(), &id, &server2.uri())
        .await
        .unwrap();
    resumed
        .send(UserMessage::text("新消息"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 3).await;
    let requests = server2.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(
        !request_user_texts(&body).contains(&"要取消的".to_owned()),
        "取消的消息不能进请求"
    );
    assert_eq!(
        count_text(
            &logged_user_texts(&read_snapshot(data_dir.path(), &id)),
            "要取消的"
        ),
        0
    );
}

#[tokio::test]
async fn invalid_persisted_inbox_is_reported_on_resume() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri()).await;
    drop(agent);
    {
        let mut store = SessionStore::open(data_dir.path(), &id).unwrap();
        // 越界 splice：start 超出空队列长度。
        store
            .append(&[EventKind::InboxSpliced {
                target: InboxTarget::NextTurn,
                start: 3,
                removed_count: None,
                inserted: vec![Message::text(Role::User, "越界")],
                outcome: None,
            }])
            .unwrap();
    }
    let error = match resume_agent(data_dir.path(), &id, &server.uri()).await {
        Ok(_) => panic!("坏掉的输入队列要让 resume 报错，不能静默丢"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, AgentError::CorruptInbox(detail) if detail.contains("inbox")),
        "要报 CorruptInbox：{error}"
    );
}

#[tokio::test]
async fn overflowing_removed_count_is_reported_as_corrupt_not_panic() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri()).await;
    drop(agent);
    {
        let mut store = SessionStore::open(data_dir.path(), &id).unwrap();
        // 先放一条待处理消息让队列长度=1；再写 start=1、removedCount=u64::MAX 的损坏
        // splice：start + removedCount 在 u64 上溢出，必须报 CorruptInbox，不能 panic。
        store
            .append(&[
                EventKind::InboxSpliced {
                    target: InboxTarget::NextTurn,
                    start: 0,
                    removed_count: None,
                    inserted: vec![Message::text(Role::User, "在队列里")],
                    outcome: None,
                },
                EventKind::InboxSpliced {
                    target: InboxTarget::NextTurn,
                    start: 1,
                    removed_count: Some(u64::MAX),
                    inserted: Vec::new(),
                    outcome: None,
                },
            ])
            .unwrap();
    }
    let error = match resume_agent(data_dir.path(), &id, &server.uri()).await {
        Ok(_) => panic!("坐标溢出的坏 splice 要让 resume 报错，不能 panic 或静默丢"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, AgentError::CorruptInbox(detail) if detail.contains("inbox")),
        "要报 CorruptInbox：{error}"
    );
}

#[tokio::test]
async fn duplicate_message_id_in_corrupt_inbox_is_reported_on_resume() {
    let server = MockServer::start().await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri()).await;
    drop(agent);
    let pending = Message::user_text("排队中");
    let pending_id = pending.id.clone();
    {
        let mut store = SessionStore::open(data_dir.path(), &id).unwrap();
        // 先合法插入；再伪造一条插入同一 MessageId 的 splice（损坏/伪造日志）。
        store
            .append(&[
                EventKind::InboxSpliced {
                    target: InboxTarget::NextTurn,
                    start: 0,
                    removed_count: None,
                    inserted: vec![pending.clone()],
                    outcome: None,
                },
                EventKind::InboxSpliced {
                    target: InboxTarget::NextTurn,
                    start: 1,
                    removed_count: None,
                    inserted: vec![pending.clone()],
                    outcome: None,
                },
            ])
            .unwrap();
    }
    let error = match resume_agent(data_dir.path(), &id, &server.uri()).await {
        Ok(_) => panic!("同一 MessageId 插两遍要让 resume 报 CorruptInbox，不能静默收下"),
        Err(error) => error,
    };
    assert!(
        matches!(&error, AgentError::CorruptInbox(detail)
            if detail.contains(pending_id.as_str()) && detail.contains("inbox")),
        "要报 CorruptInbox 且带 seq/message id：{error}"
    );
}

/// 取消（停止/关服务）照 DSH clear 清掉 next-turn 里排队的用户消息；
/// 删除落 removedCount + outcome=canceled，重启后也不再投递（页面据此把字放回输入框）。
#[tokio::test]
async fn cancel_clears_next_turn_input_durably() {
    use std::time::Duration;
    use tokio::time::{Instant, timeout_at};

    let server = MockServer::start().await;
    // 第一轮扣在模型请求上，给「运行中再排一条」留出确定窗口。
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(sse(FINAL_TEXT_SSE.to_owned()).set_delay(Duration::from_secs(10)))
        .mount(&server)
        .await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.uri()).await;
    // 等第一轮真的领取了 next-turn（InboxClaimed 通知）再排第二条：两条同时在队列里时，
    // 取消的数量取决于调度时序，测试会抖。
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let event = timeout_at(deadline, events.recv())
            .await
            .expect("等第一轮领取超时")
            .expect("事件流不应关闭");
        if let AgentEvent::InboxClaimed { message, .. } = event
            && message_text(&message) == "第一轮"
        {
            break;
        }
    }
    agent
        .send(UserMessage::text("排队的一句"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.cancel().await.unwrap();
    agent.idle().await;
    // 还有没有被取消的一轮在跑时，写锁可能还在驱动任务手里；显式关写者再 resume。
    agent.close_session_writer();

    let snapshot = read_snapshot(data_dir.path(), &id);
    let splices = splices(&snapshot);
    let canceled = splices
        .iter()
        .find(|splice| {
            splice.outcome == Some(InboxOutcome::Canceled) && splice.target == InboxTarget::NextTurn
        })
        .unwrap_or_else(|| panic!("next-turn 要被取消并落日志：{splices:?}"));
    assert_eq!(canceled.start, 0);
    assert_eq!(canceled.removed_count, Some(1));
    assert!(canceled.inserted.is_empty());
    // wire 形状：普通删除带 removedCount + outcome: canceled（DSH inbox.ts:214-221）。
    let rows = log_rows(data_dir.path(), &id);
    let wire = rows
        .iter()
        .find(|row| row["data"]["target"] == "next-turn" && row["data"]["outcome"] == "canceled")
        .unwrap_or_else(|| panic!("日志里要有 next-turn 的 canceled splice：{rows:?}"));
    assert_eq!(wire["data"]["removedCount"], 1);
    drop(snapshot);

    // 重启后队列为空：被取消的消息不再进请求、也不再落 user/message。
    drop(agent);
    let server2 = MockServer::start().await;
    mount_responses(&server2, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let resumed = resume_agent(data_dir.path(), &id, &server2.uri())
        .await
        .unwrap();
    resumed
        .send(UserMessage::text("新消息"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&resumed, 3).await;
    let requests = server2.received_requests().await.unwrap();
    let body: Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(
        !request_user_texts(&body).contains(&"排队的一句".to_owned()),
        "被取消的消息不能进请求"
    );
    assert_eq!(
        count_text(
            &logged_user_texts(&read_snapshot(data_dir.path(), &id)),
            "排队的一句"
        ),
        0
    );
}

#[tokio::test]
async fn cancel_then_restart_delivers_program_message_exactly_once() {
    let mut server = ScriptedSseServer::start(vec![
        SseScript {
            frames: PARTIAL_TEXT_SSE.to_owned(),
            hold: true,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
        SseScript {
            frames: FINAL_TEXT_SSE.to_owned(),
            hold: false,
        },
    ]);
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let (agent, id) = create_agent(data_dir.path(), cwd.path(), &server.base_url()).await;
    agent
        .send(UserMessage::text("开始"), Delivery::FollowUp)
        .await
        .unwrap();
    server.wait_ready().await;
    agent
        .send(UserMessage::text("用户排队"), Delivery::FollowUp)
        .await
        .unwrap();
    agent
        .send(
            UserMessage::text_with_request_id("任务报回了", "task-report:t:r"),
            Delivery::FollowUp,
        )
        .await
        .unwrap();
    agent.cancel().await.unwrap();
    wait_idle(&agent, 3).await;
    drop(agent);

    let resumed = resume_agent(data_dir.path(), &id, &server.base_url())
        .await
        .expect("取消后能 resume");
    resumed
        .send(UserMessage::text("恢复后新话"), Delivery::FollowUp)
        .await
        .unwrap();
    server.release();
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(10), resumed.idle())
            .await
            .is_ok(),
        "resume 后 idle() 必须返回"
    );

    let bodies = server.request_bodies();
    assert_eq!(
        bodies.len(),
        3,
        "aborted 后程序消息一轮 + 新话一轮，共三次请求"
    );
    let program: Value = serde_json::from_slice(&bodies[1]).unwrap();
    assert_eq!(
        request_user_texts(&program).last().map(String::as_str),
        Some("任务报回了")
    );
    let after: Value = serde_json::from_slice(&bodies[2]).unwrap();
    assert_eq!(
        request_user_texts(&after).last().map(String::as_str),
        Some("恢复后新话")
    );
    for body in &bodies {
        let body: Value = serde_json::from_slice(body).unwrap();
        assert!(
            !request_user_texts(&body)
                .iter()
                .any(|text| text == "用户排队"),
            "被取消的用户排队话不能再出现"
        );
    }

    // 再下一轮：程序消息不再重复。
    resumed
        .send(UserMessage::text("再下一句"), Delivery::FollowUp)
        .await
        .unwrap();
    resumed.idle().await;
    let bodies = server.request_bodies();
    let last: Value = serde_json::from_slice(bodies.last().unwrap()).unwrap();
    assert_eq!(
        request_user_texts(&last).last().map(String::as_str),
        Some("再下一句")
    );
    let snapshot = read_snapshot(data_dir.path(), &id);
    let texts = logged_user_texts(&snapshot);
    assert_eq!(
        count_text(&texts, "任务报回了"),
        1,
        "程序消息恰好投递一次：{texts:?}"
    );
    assert_eq!(
        count_text(&texts, "用户排队"),
        0,
        "被取消的用户排队话不进日志：{texts:?}"
    );
}
