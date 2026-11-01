//! T005.10：ask_user_question 问题事件、answer 续跑、等待中 cancel 撤销（测试清单第 14 条）。

mod common;

use std::time::Duration;

use agent::{Agent, AgentEvent, Delivery, TurnOutcome};
use common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse,
    tool_calls_with_inputs_sse, wait_idle,
};
use model::UserMessage;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn setup(server: &MockServer) -> (Agent, tempfile::TempDir, session::SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = session::SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::new(model_config(&server.uri())).unwrap(),
            tools: tools::ToolSet::builtin(),
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

fn ask_call(question_id: &str) -> String {
    tool_calls_with_inputs_sse(&[(
        "call_ask",
        "ask_user_question",
        json!({
            "questions": [{
                "id": question_id,
                "question": "Proceed with cleanup?",
                "header": "Confirm",
                "options": [
                    {"label": "Yes", "description": "go"},
                    {"label": "No"}
                ]
            }]
        }),
    )])
}

async fn next_question(events: &mut tokio::sync::broadcast::Receiver<AgentEvent>) -> AgentEvent {
    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await.expect("事件流不应关闭") {
                event @ AgentEvent::QuestionAsked { .. } => break event,
                _ => continue,
            }
        }
    })
    .await;
    assert!(received.is_ok(), "应发出 QuestionAsked 且 loop 停住");
    received.unwrap()
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
async fn question_event_stops_loop_until_answer_then_resumes() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q1"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, _data, _id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    let AgentEvent::QuestionAsked { question } = next_question(&mut events).await else {
        unreachable!()
    };
    assert_eq!(question.id, "q1");
    assert_eq!(question.header.as_deref(), Some("Confirm"));
    assert_eq!(question.options.len(), 2);
    assert_eq!(bodies(&server).await.len(), 1, "等待时不应再发模型请求");
    assert!(!agent.answer("wrong-id", "x").await, "id 不绑定不可答");
    assert!(agent.answer("q1", "Yes").await);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "answer 后 loop 接着跑");
    assert!(bodies[1].contains("Yes"), "答案作为工具结果进下一个请求");
    let mut ended = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome, .. } = event {
            ended = Some(outcome);
        }
    }
    assert_eq!(ended, Some(TurnOutcome::Completed));
}

#[tokio::test]
async fn answer_continues_same_turn_and_logs_question() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q2"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    let AgentEvent::QuestionAsked { question } = next_question(&mut events).await else {
        unreachable!()
    };
    assert!(agent.answer(&question.id, "Yes").await);
    wait_idle(&agent, 20).await;
    let store = session::SessionStore::open(data_dir.path(), &id).unwrap();
    let events = store.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::TurnStart { .. }))
            .count(),
        1,
        "answer 后在同一轮继续"
    );
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. }))
    );
}

#[tokio::test]
async fn cancel_while_waiting_revokes_question_and_aborts_turn() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q3"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent.send(UserMessage::text("go"), Delivery::FollowUp);
    let AgentEvent::QuestionAsked { question } = next_question(&mut events).await else {
        unreachable!()
    };
    agent.cancel();
    wait_idle(&agent, 20).await;
    assert_eq!(bodies(&server).await.len(), 1, "取消后不再发请求");
    assert!(
        !agent.answer(&question.id, "Yes").await,
        "撤销后答案不再接收"
    );
    let store = session::SessionStore::open(data_dir.path(), &id).unwrap();
    let events = store.events();
    assert!(
        events
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::QuestionCancelled { .. }))
    );
    let ended = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
            _ => None,
        })
        .expect("应有 turn/end");
    assert_eq!(ended, session::TurnEndReason::Aborted);
}
