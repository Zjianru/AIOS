//! 接缝 1：每轮 step 编号从 1 开始，不跨 turn 携带（T004 收尾 K1 回归）。
//!
//! 三种状态只有在「同一驱动任务连续开下一轮」时才暴露：轮末已有排队的
//! follow-up 时，kick 直接进下一轮；旧实现把上一轮的 step 计数带过去。

mod common;

use std::time::Duration;

use agent::{Agent, AgentConfig, Delivery, Deps};
use common::{FINAL_TEXT_SSE, SequenceResponder, ToolEvent, gated, sse, tool_calls_sse};
use model::{DeepSeek, DeepSeekConfig, Effort, UserMessage};
use session::{EventKind, SessionId, SessionStore};
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

async fn setup(
    base_url: &str,
) -> (
    Agent,
    tempfile::TempDir,
    tempfile::TempDir,
    SessionId,
    mpsc::UnboundedReceiver<ToolEvent>,
    Vec<std::sync::Arc<tokio::sync::Semaphore>>,
) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let (started_tx, started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let permits: Vec<_> = permits.into_iter().map(|(_, permit)| permit).collect();
    let agent = Agent::create(
        AgentConfig::default(),
        Deps {
            model: DeepSeek::new(model_config(base_url)).unwrap(),
            tools,
            store,
        },
    )
    .await
    .unwrap();
    (agent, data_dir, cwd, id, started, permits)
}

async fn next_started(started: &mut mpsc::UnboundedReceiver<ToolEvent>) -> String {
    loop {
        match started.recv().await.expect("工具事件通道关闭") {
            ToolEvent::Started(name) => return name,
            ToolEvent::Finished(_) => {}
        }
    }
}

async fn wait_idle(agent: &Agent) {
    tokio::time::timeout(Duration::from_secs(10), agent.idle())
        .await
        .expect("idle() 超时");
}

fn step_numbers(store: &SessionStore) -> Vec<(u64, u64)> {
    store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::StepStart { turn, step } => Some((*turn, *step)),
            _ => None,
        })
        .collect()
}

fn turn_reasons(store: &SessionStore) -> Vec<session::TurnEndReason> {
    store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn each_turn_starts_step_numbering_at_one() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![
            sse(tool_calls_sse(&[("call_0", "gate")])),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(tool_calls_sse(&[("call_1", "gate")])),
            sse(FINAL_TEXT_SSE.to_owned()),
            sse(FINAL_TEXT_SSE.to_owned()),
        ]))
        .mount(&server)
        .await;
    let (agent, data_dir, _cwd, id, mut started, permits) = setup(&server.uri()).await;

    agent.send(UserMessage::text("第一轮"), Delivery::FollowUp);
    assert_eq!(next_started(&mut started).await, "gate");
    // 第一轮跑工具时排一条 follow-up：轮末已有 pending，kick 直接开第二轮。
    agent.send(UserMessage::text("第二轮"), Delivery::FollowUp);
    permits[0].add_permits(1);
    assert_eq!(next_started(&mut started).await, "gate");
    // 第二轮照样排第三轮的 follow-up。
    agent.send(UserMessage::text("第三轮"), Delivery::FollowUp);
    permits[0].add_permits(1);
    wait_idle(&agent).await;

    let store = SessionStore::open(data_dir.path(), &id).unwrap();
    assert_eq!(
        step_numbers(&store),
        vec![(1, 1), (1, 2), (2, 1), (2, 2), (3, 1)],
        "每轮 step 编号都从 1 开始，不能跨 turn 携带旧值"
    );
    assert_eq!(
        turn_reasons(&store),
        vec![
            session::TurnEndReason::Completed,
            session::TurnEndReason::Completed,
            session::TurnEndReason::Completed,
        ]
    );
}
