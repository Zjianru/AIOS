//! T006.13：ask_user_question 改回 DSH 语义——答案 `{id, selected[], custom?}`（K14）、
//! 回答方接缝（`Agent::cancel_question`）、问题/答案的 durable 事件、等待中撤销，
//! 以及待答问题跨重启保留后以迟到回答消息接续（D45）。
//!
//! DSH 依据：`packages/interaction/tool-ask-user/src/index.ts:78-118`（工具输出 schema 与
//! execute 映射）、`user-questions/src/index.ts:162-210`（answer 只接受 continued 问题、
//! 批必须每题恰好一次、迟到回答以 user-question-reply 消息 steer）、
//! `user-questions/src/types.ts:72-122`（答案项与回复 source）。

use std::time::Duration;

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse,
    tool_calls_with_inputs_sse, wait_idle,
};
use agent::{Agent, AgentEvent, Delivery, TurnOutcome};
use model::UserMessage;
use serde_json::json;
use session::{EventKind, SessionId, SessionStore, TurnEndReason};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

async fn setup(server: &MockServer) -> (Agent, tempfile::TempDir, SessionId) {
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                crate::common::test_credential(),
            )
            .unwrap(),
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

fn log_path(data_dir: &std::path::Path, id: &SessionId) -> std::path::PathBuf {
    data_dir.join("sessions").join(format!("{id}.jsonl"))
}

fn logged_events(data_dir: &std::path::Path, id: &SessionId) -> Vec<session::Event> {
    SessionStore::open_read_only(data_dir, id)
        .unwrap()
        .events()
        .to_vec()
}

async fn resume_agent(data_dir: &std::path::Path, id: &SessionId, base_url: &str) -> Agent {
    let store = SessionStore::open(data_dir, id).unwrap();
    Agent::resume(
        id.clone(),
        isolated_config(data_dir),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config(base_url),
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap()
}

/// 把崩溃前的日志前缀抄到新 data-dir（模拟内核被杀后重启：新进程从同一日志恢复）。
fn copy_crashed_log(data_dir: &std::path::Path, id: &SessionId) -> tempfile::TempDir {
    let crashed_prefix = std::fs::read(log_path(data_dir, id)).unwrap();
    let crashed_dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(crashed_dir.path().join("sessions")).unwrap();
    std::fs::write(log_path(crashed_dir.path(), id), crashed_prefix).unwrap();
    crashed_dir
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

/// 请求里的迟到回答文本（DSH 迟到回答是一条普通 user 消息，内容是 JSON 文本）。
fn late_reply_text(body: &serde_json::Value) -> Option<String> {
    for message in body["messages"].as_array()? {
        if message["role"] != "user" {
            continue;
        }
        for block in message["content"].as_array()? {
            if block["type"] != "text" {
                continue;
            }
            let text = block["text"].as_str()?;
            if text.contains("answer_to_pending_question") {
                return Some(text.to_owned());
            }
        }
    }
    None
}

#[tokio::test]
async fn question_event_stops_loop_until_answer_then_resumes() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q1"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, id) = setup(&server).await;
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
    assert!(agent.answer("q1", vec!["Yes".to_owned()], None).await);
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "answer 后 loop 接着跑");
    let payload = tool_result_payload(&bodies[1], "call_ask");
    assert_eq!(
        payload,
        json!({"answers": [{"id": "q1", "selected": ["Yes"]}]}),
        "工具结果照 DSH：answers[]{{id, selected[], custom?}}（tool-ask-user/src/index.ts:111-116）"
    );

    // 问题与答案都有 durable 事件；question/asked 带 callId 指向这次调用。
    let logged = logged_events(data_dir.path(), &id);
    let asked = logged.iter().find_map(|event| match &event.kind {
        EventKind::QuestionAsked {
            call_id,
            question_id,
            ..
        } => Some((call_id.clone(), question_id.clone())),
        _ => None,
    });
    assert_eq!(
        asked,
        Some(("call_ask".to_owned(), "q1".to_owned())),
        "question/asked 要带 callId（重启折叠用）"
    );
    let answered = logged.iter().find_map(|event| match &event.kind {
        EventKind::QuestionAnswered {
            question_id,
            selected,
            custom,
            ..
        } => Some((question_id.clone(), selected.clone(), custom.clone())),
        _ => None,
    });
    assert_eq!(
        answered,
        Some(("q1".to_owned(), vec!["Yes".to_owned()], None)),
        "回答也写 durable 的 question/answered（selected 保持标签、无 custom）"
    );

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
            .answer(&questions[0].id, vec!["No".to_owned()], None)
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
            .answer(&questions[0].id, vec!["Yes".to_owned()], None)
            .await,
        "撤销后答案不再接收"
    );
    let events = logged_events(data_dir.path(), &id);
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

#[tokio::test]
async fn two_questions_answered_as_dsh_batch_option_and_custom() {
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
    assert!(agent.answer("q1", vec!["Yes".to_owned()], None).await);
    assert_eq!(bodies(&server).await.len(), 1, "未答完不应生成 tool/result");
    assert!(
        agent
            .answer("q2", Vec::new(), Some("我自己的答案".to_owned()))
            .await
    );
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "全部答完后继续跑");
    let payload = tool_result_payload(&bodies[1], "call_ask");
    assert_eq!(
        payload,
        json!({"answers": [
            {"id": "q1", "selected": ["Yes"]},
            {"id": "q2", "selected": [], "custom": "我自己的答案"}
        ]}),
        "一题选项、一题自定义，批 shape 照 DSH（K14 / tool-ask-user/src/index.ts:111-116）"
    );
    let events = logged_events(data_dir.path(), &id);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. }))
            .count(),
        2,
        "两个问题各一条 question/asked"
    );
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAnswered { .. }))
            .count(),
        2,
        "两个答案各一条 question/answered"
    );
}

#[tokio::test]
async fn dismiss_question_returns_ask_aborted_and_turn_continues() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q1"), FINAL_TEXT_SSE.to_owned()]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { .. } = next_question(&mut events).await else {
        unreachable!()
    };
    assert!(
        agent.cancel_question("q1").await,
        "提问服务路径撤销应命中待答问题"
    );
    wait_idle(&agent, 20).await;
    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "撤销后这一步继续跑（不是整轮取消）");
    assert!(
        bodies[1].contains("ask_user_question was aborted before the user answered"),
        "模型收到 DSH 的 ASK_ABORTED 文案（user-questions/src/index.ts:53-59）：{}",
        bodies[1]
    );
    let mut ended = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome, .. } = event {
            ended = Some(outcome);
        }
    }
    assert_eq!(
        ended,
        Some(TurnOutcome::Completed),
        "撤销问题不是取消整轮（DSH：工具错误、本轮继续）"
    );
    let events = logged_events(data_dir.path(), &id);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionCancelled { .. }))
            .count(),
        1,
        "撤销写 question/cancelled"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.kind, session::EventKind::QuestionAnswered { .. })),
        "没有答案就不写 question/answered"
    );
    let error_code = events.iter().find_map(|event| match &event.kind {
        EventKind::ToolResult { message, error, .. }
            if message.tool_call_id.as_deref() == Some("call_ask") =>
        {
            error.as_ref().map(|error| error.code.clone())
        }
        _ => None,
    });
    assert_eq!(
        error_code.as_deref(),
        Some("ASK_ABORTED"),
        "工具结果错误码照 DSH HarnessError code"
    );
    assert!(
        !agent.answer("q1", vec!["Yes".to_owned()], None).await,
        "撤销后答案不再接收"
    );
}

#[tokio::test]
async fn pending_questions_survive_restart_and_answer_continues_as_late_reply() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q1")]).await;
    let data_dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = SessionStore::create(data_dir.path(), cwd.path()).unwrap();
    let id = store.id().clone();
    let agent = Agent::create(
        isolated_config(data_dir.path()),
        agent::Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                crate::common::test_credential(),
            )
            .unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { questions } = next_question(&mut events).await else {
        unreachable!()
    };
    assert_eq!(questions[0].id, "q1");
    // 杀掉进程：日志前缀拷到新 data-dir，旧 agent 的驱动任务留在旧目录上不再写。
    let crashed_dir = copy_crashed_log(data_dir.path(), &id);
    drop(agent);

    let resumed_server = MockServer::start().await;
    mount(&resumed_server, vec![FINAL_TEXT_SSE.to_owned()]).await;
    let resumed = resume_agent(crashed_dir.path(), &id, &resumed_server.uri()).await;
    assert_eq!(
        resumed.pending_questions().len(),
        1,
        "D45：内核重启后待答问题照 DSH 保留"
    );
    assert_eq!(resumed.pending_questions()[0].id, "q1");
    assert!(
        resumed.answer("q1", vec!["Yes".to_owned()], None).await,
        "恢复的待答问题可以通过回答接缝接续"
    );
    assert!(
        !resumed.answer("q1", vec!["Yes".to_owned()], None).await,
        "同一个问题不能答两次（DSH REPLY_QUEUED / 已结算即拒绝）"
    );
    wait_idle(&resumed, 20).await;
    assert!(resumed.pending_questions().is_empty());

    // 模型下一轮看到 DSH 的迟到回答消息（user-questions/src/index.ts:191-199）。
    let requests = resumed_server.received_requests().await.unwrap();
    assert_eq!(requests.len(), 1, "回答恢复的问题要唤醒一轮");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).unwrap();
    assert!(
        body["messages"]
            .as_array()
            .is_some_and(
                |messages| messages.iter().any(|message| message["role"] == "assistant"
                    && message["content"].as_array().is_some_and(|content| content
                        .iter()
                        .any(|block| block["type"] == "tool_use" && block["id"] == "call_ask")))
            ),
        "历史里保留原 ask 调用：{body}"
    );
    let text = late_reply_text(&body).expect("请求里要有迟到回答消息");
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        json!({
            "kind": "answer_to_pending_question",
            "tool": "ask_user_question",
            "callId": "call_ask",
            "questions": [{
                "id": "q1",
                "question": "Proceed with cleanup?",
                "header": "Confirm",
                "options": [
                    {"label": "Yes", "description": "go"},
                    {"label": "No"}
                ]
            }],
            "answers": [{"id": "q1", "selected": ["Yes"]}]
        }),
        "迟到回答 JSON 照 DSH（questions 用服务词汇；answers 用答案项）"
    );

    let events = logged_events(crashed_dir.path(), &id);
    assert!(
        events.iter().any(|event| matches!(
            event.kind,
            EventKind::TurnEnd {
                reason: TurnEndReason::Interrupted,
                ..
            }
        )),
        "重启先做中断修复（T006.5a repair）"
    );
    assert!(
        events.iter().any(|event| matches!(
            &event.kind,
            EventKind::QuestionAnswered { question_id, .. } if question_id == "q1"
        )),
        "恢复后回答也写 question/answered"
    );
    let reply_source = events.iter().find_map(|event| match &event.kind {
        EventKind::UserMessage { message, .. } => match &message.source {
            model::MessageSource::UserQuestionReply { call_id } => Some(call_id.clone()),
            _ => None,
        },
        _ => None,
    });
    assert_eq!(
        reply_source.as_deref(),
        Some("call_ask"),
        "迟到回答的 user/message 带 user-question-reply source（DSH）"
    );
}

#[tokio::test]
async fn discarded_restored_question_is_not_asked_again_after_next_restart() {
    let server = MockServer::start().await;
    mount(&server, vec![ask_call("q1")]).await;
    let (agent, data_dir, id) = setup(&server).await;
    let mut events = agent.subscribe();
    agent
        .send(UserMessage::text("go"), Delivery::FollowUp)
        .await
        .unwrap();
    let AgentEvent::QuestionAsked { .. } = next_question(&mut events).await else {
        unreachable!()
    };
    let crashed_dir = copy_crashed_log(data_dir.path(), &id);
    drop(agent);

    let resumed_server = MockServer::start().await;
    let resumed = resume_agent(crashed_dir.path(), &id, &resumed_server.uri()).await;
    assert_eq!(resumed.pending_questions().len(), 1);
    assert!(
        resumed.cancel_question("q1").await,
        "重启后的待答问题也能走提问服务撤销"
    );
    assert!(resumed.pending_questions().is_empty());
    assert!(
        resumed_server.received_requests().await.unwrap().is_empty(),
        "撤销不唤醒新请求"
    );
    let events = logged_events(crashed_dir.path(), &id);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event.kind, EventKind::QuestionCancelled { .. }))
            .count(),
        1,
        "撤销写 durable 的 question/cancelled"
    );
    drop(resumed);

    // 再重启一次：撤销过的问题不能复活（D45 折叠语义）。
    let again = resume_agent(crashed_dir.path(), &id, &resumed_server.uri()).await;
    assert!(again.pending_questions().is_empty(), "撤销记录要跨重启生效");
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
    assert!(agent.answer("q1", vec!["Yes".to_owned()], None).await);
    agent.cancel().await.unwrap();
    wait_idle(&agent, 20).await;
    assert_eq!(bodies(&server).await.len(), 1, "取消后不再发请求");
    let events = logged_events(data_dir.path(), &id);
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
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAnswered { .. }))
            .count(),
        1,
        "已答的那一题留下 durable 的 question/answered"
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

fn two_distinct_ask_calls() -> String {
    tool_calls_with_inputs_sse(&[
        (
            "call_ask1",
            "ask_user_question",
            json!({"questions": [{"id": "qa", "question": "第一问？", "options": [{"label": "Yes"}, {"label": "No"}]}]}),
        ),
        (
            "call_ask2",
            "ask_user_question",
            json!({"questions": [{"id": "qb", "question": "第二问？", "options": [{"label": "Yes"}, {"label": "No"}]}]}),
        ),
    ])
}

async fn next_question_in_batch(
    events: &mut tokio::sync::broadcast::Receiver<AgentEvent>,
) -> AgentEvent {
    let received = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match events.recv().await.expect("事件流不应关闭") {
                event @ AgentEvent::QuestionAsked { .. } => break event,
                _ => continue,
            }
        }
    })
    .await;
    received.expect("批内第二个 ask 也应依次拦截（T006.13b）")
}

/// T006.13b：一个 assistant batch 里的多个 ask_user_question 按模型顺序依次拦截：
/// 先问 qa，答完再问 qb，最后两个 tool/result 按模型顺序回填、本轮 completed。
/// DSH 依据：tool-ask-user 每个调用独立走 userQuestions.ask（tool-ask-user/src/index.ts:99-118），
/// 批执行按调用顺序串行（独占工具），因此多 ask 是「依次提问/依次作答」，不是并行弹窗。
#[tokio::test]
async fn multiple_ask_calls_in_one_batch_are_intercepted_in_order() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![two_distinct_ask_calls(), FINAL_TEXT_SSE.to_owned()],
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
    assert_eq!(questions[0].id, "qa", "先截获批内第一个 ask");
    assert!(agent.answer("qa", vec!["Yes".to_owned()], None).await);
    let AgentEvent::QuestionAsked { questions } = next_question_in_batch(&mut events).await else {
        unreachable!()
    };
    assert_eq!(questions[0].id, "qb", "第一个答完再截获第二个 ask");
    assert!(agent.answer("qb", vec!["No".to_owned()], None).await);
    wait_idle(&agent, 20).await;

    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "两个 ask 都在同一个 step 内被拦截");
    let body = &bodies[1];
    let first = tool_result_payload(body, "call_ask1");
    let second = tool_result_payload(body, "call_ask2");
    assert_eq!(
        first,
        json!({"answers": [{"id": "qa", "selected": ["Yes"]}]})
    );
    assert_eq!(
        second,
        json!({"answers": [{"id": "qb", "selected": ["No"]}]})
    );
    let first_at = body.find("call_ask1").expect("第一个结果");
    let second_at = body.find("call_ask2").expect("第二个结果");
    assert!(first_at < second_at, "tool/result 按模型顺序");
    assert!(
        !body.contains("ASK_USER_LOOP_ONLY"),
        "第二个 ask 不能再落到内置工具 stub：{body}"
    );

    let logged = logged_events(data_dir.path(), &id);
    assert_eq!(
        logged
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAsked { .. }))
            .count(),
        2,
        "两个 ask 各一条 question/asked"
    );
    assert_eq!(
        logged
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionAnswered { .. }))
            .count(),
        2,
        "两个答案各一条 question/answered"
    );
    let mut events = events;
    let mut ended = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome, .. } = event {
            ended = Some(outcome);
        }
    }
    assert_eq!(ended, Some(TurnOutcome::Completed), "多 ask 后本轮正常完成");
}

/// T006.13b：批内第二个 ask 被提问服务路径撤销时，第一个的答案保留、第二个回
/// DSH ASK_ABORTED、本轮继续（不是整轮取消）。
#[tokio::test]
async fn dismiss_second_ask_mid_batch_returns_ask_aborted_and_turn_continues() {
    let server = MockServer::start().await;
    mount(
        &server,
        vec![two_distinct_ask_calls(), FINAL_TEXT_SSE.to_owned()],
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
    assert_eq!(questions[0].id, "qa");
    assert!(agent.answer("qa", vec!["Yes".to_owned()], None).await);
    let AgentEvent::QuestionAsked { questions } = next_question_in_batch(&mut events).await else {
        unreachable!()
    };
    assert_eq!(questions[0].id, "qb");
    assert!(agent.cancel_question("qb").await, "撤销第二个 ask");
    wait_idle(&agent, 20).await;

    let bodies = bodies(&server).await;
    assert_eq!(bodies.len(), 2, "撤销后本轮继续");
    let body = &bodies[1];
    let first = tool_result_payload(body, "call_ask1");
    assert_eq!(
        first,
        json!({"answers": [{"id": "qa", "selected": ["Yes"]}]})
    );
    assert!(
        body.contains("ask_user_question was aborted before the user answered"),
        "第二个 ask 收到 DSH ASK_ABORTED：{body}"
    );
    let logged = logged_events(data_dir.path(), &id);
    assert_eq!(
        logged
            .iter()
            .filter(|event| matches!(event.kind, session::EventKind::QuestionCancelled { .. }))
            .count(),
        1,
        "撤销写一条 question/cancelled"
    );
    let mut ended = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::TurnEnded { outcome, .. } = event {
            ended = Some(outcome);
        }
    }
    assert_eq!(ended, Some(TurnOutcome::Completed), "撤销问题不是取消整轮");
}
