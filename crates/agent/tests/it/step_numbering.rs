//! 接缝 1：每轮 step 编号从 1 开始，不跨 turn 携带（T004 收尾 K1 回归）。
//!
//! 三种状态只有在「同一驱动任务连续开下一轮」时才暴露：轮末已有排队的
//! follow-up 时，kick 直接进下一轮；旧实现把上一轮的 step 计数带过去。



use agent::Delivery;
use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, ToolEvent, gated, setup_agent, sse, tool_calls_sse,
    wait_idle,
};
use model::UserMessage;
use session::{EventKind, SessionStore};
use tokio::sync::mpsc;
use tools::{ToolConcurrency, ToolSet};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn next_started(started: &mut mpsc::UnboundedReceiver<ToolEvent>) -> String {
    loop {
        match started.recv().await.expect("工具事件通道关闭") {
            ToolEvent::Started(name) => return name,
            ToolEvent::Finished(_) => {}
        }
    }
}

fn step_numbers(store: &session::SessionSnapshot) -> Vec<(u64, u64)> {
    store
        .events()
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::StepStart { turn, step } => Some((*turn, *step)),
            _ => None,
        })
        .collect()
}

fn turn_reasons(store: &session::SessionSnapshot) -> Vec<session::TurnEndReason> {
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
    let (started_tx, mut started) = mpsc::unbounded_channel();
    let mut permits = Vec::new();
    let tools = ToolSet::new(vec![gated(
        "gate",
        ToolConcurrency::Parallel,
        &started_tx,
        &mut permits,
    )]);
    let permits: Vec<_> = permits.into_iter().map(|(_, permit)| permit).collect();
    let (agent, data_dir, _cwd, id) = setup_agent(&server.uri(), tools).await;

    agent
        .send(UserMessage::text("第一轮"), Delivery::FollowUp)
        .await
        .unwrap();
    assert_eq!(next_started(&mut started).await, "gate");
    // 第一轮跑工具时排一条 follow-up：轮末已有 pending，kick 直接开第二轮。
    agent
        .send(UserMessage::text("第二轮"), Delivery::FollowUp)
        .await
        .unwrap();
    permits[0].add_permits(1);
    assert_eq!(next_started(&mut started).await, "gate");
    // 第二轮照样排第三轮的 follow-up。
    agent
        .send(UserMessage::text("第三轮"), Delivery::FollowUp)
        .await
        .unwrap();
    permits[0].add_permits(1);
    wait_idle(&agent, 10).await;

    let store = SessionStore::open_read_only(data_dir.path(), &id).unwrap();
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
