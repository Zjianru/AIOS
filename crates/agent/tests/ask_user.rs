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
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { questions } = next_question(&mut events).await else {
        unreachable!()
    };
    let question = &questions[0];
    assert_eq!(question.id, "q1");
    assert_eq!(question.header.as_deref(), Some("Confirm"));
    assert_eq!(question.options.len(), 2);
    assert_eq!(bodies(&server).await.len(), 1, "等待时不应再发模型请求");
    assert!(
        !agent
            .answer("wrong-id", Vec::new(), Some("x".to_owned()))
            .await,
        "id 不绑定不可答"
    );
    assert!(agent.answer("q1", Vec::new(), Some("Yes".to_owned())).await);
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
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { questions } = next_question(&mut events).await else {
        unreachable!()
    };
    assert!(
        agent
            .answer(&questions[0].id, Vec::new(), Some("Yes".to_owned()))
            .await
    );
    wait_idle(&agent, 20).await;
    let store = session::SessionStore::open_read_only(data_dir.path(), &id).unwrap();
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
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { questions } = next_question(&mut events).await else {
        unreachable!()
    };
    agent.cancel().await.unwrap();
    wait_idle(&agent, 20).await;
    assert_eq!(bodies(&server).await.len(), 1, "取消后不再发请求");
    assert!(
        !agent
            .answer(&questions[0].id, Vec::new(), Some("Yes".to_owned()))
            .await,
        "撤销后答案不再接收"
    );
    let store = session::SessionStore::open_read_only(data_dir.path(), &id).unwrap();
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
    assert_eq!(
        ended,
        session::TurnEndReason::Aborted {
            reason: session::AbortCause::User
        }
    );
}

fn ask_call_multi(ids: &[&str]) -> String {
    let questions: Vec<serde_json::Value> = ids
        .iter()
        .enumerate()
        .map(|(index, id)| {
            json!({
                "id": id,
                "question": format!("Question {index}?"),
                "header": format!("H{index}"),
                "options": [{"label": "Yes"}, {"label": "No"}]
            })
        })
        .collect();
    tool_calls_with_inputs_sse(&[(
        "call_ask",
        "ask_user_question",
        json!({"questions": questions}),
    )])
}

#[tokio::test]
async fn two_questions_are_all_delivered_and_answered() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![ask_call_multi(&["q1", "q2"]), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { questions } = next_question(&mut events).await else {
        unreachable!()
    };
    assert_eq!(questions.len(), 2, "两个问题都要送达：{questions:?}");
    assert_eq!(questions[0].id, "q1");
    assert_eq!(questions[1].id, "q2");
    assert!(
        agent
            .answer("q1", Vec::new(), Some("ANSWER-ONE".to_owned()))
            .await
    );
    assert_eq!(bodies(&server).await.len(), 1, "未答完不应生成 tool/result");
    assert!(
        agent
            .answer("q2", Vec::new(), Some("ANSWER-TWO".to_owned()))
            .await
    );
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "全部答完后继续跑");
    let payload = tool_result_payload(&bodies[1], "call_ask");
    let answers = payload["answers"].as_array().expect("answers 数组");
    assert_eq!(answers.len(), 2, "两个答案都要回到工具结果");
    assert_eq!(answers[0]["id"].as_str(), Some("q1"));
    assert_eq!(answers[0]["answer"].as_str(), Some("ANSWER-ONE"));
    assert_eq!(answers[1]["id"].as_str(), Some("q2"));
    assert_eq!(answers[1]["answer"].as_str(), Some("ANSWER-TWO"));
    let store = session::SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert_eq!(
        store
            .events()
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. }))
            .count(),
        2,
        "两个问题各一条 question/asked"
    );
}

#[tokio::test]
async fn invalid_ask_goes_through_unified_tool_result_path() {
    let server = MockServer::start().await;
    let response = tool_calls_with_inputs_sse(&[
        (
            "call_todo",
            "todo_write",
            json!({"todos": [{"content": "step", "status": "pending"}]}),
        ),
        (
            "call_ask",
            "ask_user_question",
            json!({"questions": [{"id": "q-bad"}]}),
        ),
    ]);
    mount(&server, vec![response, FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2);
    assert!(
        bodies[1].contains("ASK_USER_INVALID") || bodies[1].contains("invalid ask_user_question")
    );
    let store = session::SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert!(
        store
            .events()
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::TodoWrite { .. })),
        "invalid ask 也要走统一 post-batch：同批 todo/write 不能丢"
    );
    assert!(
        !store
            .events()
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. }))
    );
    let mut ended = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome, .. } = event {
            ended = Some(outcome);
        }
    }
    assert_eq!(ended, Some(TurnOutcome::Completed));
}

fn tool_result_payload(body: &str, call_id: &str) -> serde_json::Value {
    let request: serde_json::Value = serde_json::from_str(body).expect("请求 body 是 JSON");
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
async fn duplicate_question_ids_reject_whole_call() {
    let server = MockServer::start().await;
    let response = tool_calls_with_inputs_sse(&[(
        "call_ask",
        "ask_user_question",
        json!({"questions": [{"id": "dup", "question": "A?"}, {"id": "dup", "question": "B?"}]}),
    )]);
    mount(&server, vec![response, FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, id) = setup(&server).await;
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "重复 id 应整单拒绝并继续本轮，不悬挂");
    assert!(
        bodies[1].contains("ASK_USER_INVALID") || bodies[1].contains("invalid ask_user_question")
    );
    let store = session::SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    assert!(
        !store
            .events()
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. })),
        "整单拒绝时不应发问题事件"
    );
}

#[tokio::test]
async fn cancel_after_partial_answers_does_not_hang() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![ask_call_multi(&["q1", "q2"]), FINAL_TEXT_SSE.to_owned()],
    )
    .await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { questions } = next_question(&mut events).await else {
        unreachable!()
    };
    assert_eq!(questions.len(), 2);
    assert!(
        agent
            .answer("q1", Vec::new(), Some("ONLY-ONE".to_owned()))
            .await
    );
    agent.cancel().await.unwrap();
    wait_idle(&agent, 20).await;
    assert_eq!(bodies(&server).await.len(), 1, "取消后不再发请求");
    let store = session::SessionStore::open_read_only(data_dir.path(), &id).unwrap();
    let events = store.events();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. }))
            .count(),
        2
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionCancelled { .. }))
            .count(),
        1,
        "只撤销未答的那一题"
    );
}

#[tokio::test]
async fn cancel_turn_ends_aborted_not_error() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q5"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, _data_dir, _id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { .. } = next_question(&mut events).await else {
        unreachable!()
    };
    agent.cancel().await.unwrap();
    wait_idle(&agent, 20).await;
    let mut outcome: Option<TurnOutcome> = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome: ended, .. } = event {
            outcome = Some(ended);
        }
    }
    assert!(
        matches!(outcome, Some(TurnOutcome::Aborted)),
        "取消应由 StepEnd::Aborted 收尾，不是 error：{outcome:?}"
    );
}
