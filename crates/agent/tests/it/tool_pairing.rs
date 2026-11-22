//! 组模型请求前丢孤儿 tool call/result：取消的问题不能把后续每轮都打成 400。

use crate::common::{
    FINAL_TEXT_SSE, SequenceResponder, isolated_config, model_config, sse, test_credential,
};
use agent::{Agent, Delivery, Deps};
use model::{ContentBlock, Message, UserMessage};
use session::{EventKind, SessionStore};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer};

fn assistant_with_call(text: &str, call_id: &str) -> EventKind {
    EventKind::AssistantMessage {
        turn: 1,
        step: 1,
        message: Message::assistant(
            vec![
                ContentBlock::text(text),
                ContentBlock::ToolCall {
                    id: call_id.to_owned(),
                    name: "ask_user_question".to_owned(),
                    arguments: "{}".to_owned(),
                },
            ],
            "deepseek",
            "deepseek-flash",
        ),
        stream: Vec::new(),
        usage: None,
        interrupted: None,
        effort: None,
        effort_wanted: None,
        request_at: None,
        context_parts: None,
    }
}

async fn request_body_with(session_events: Vec<EventKind>) -> String {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(SequenceResponder::new(vec![sse(FINAL_TEXT_SSE.to_owned())]))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut store = SessionStore::create(data.path(), project.path()).unwrap();
    store.append(&session_events).unwrap();
    let id = store.id().clone();
    let agent = Agent::resume(
        id,
        isolated_config(data.path()),
        Deps {
            model: model::DeepSeek::with_credentials(
                model_config(&server.uri()),
                test_credential(),
            )
            .unwrap(),
            tools: tools::ToolSet::builtin(),
            store,
        },
    )
    .await
    .unwrap();
    agent
        .send(UserMessage::text("继续"), Delivery::FollowUp)
        .await
        .unwrap();
    agent.idle().await;
    let requests = server.received_requests().await.unwrap();
    String::from_utf8(requests.last().unwrap().body.clone()).unwrap()
}

#[tokio::test]
async fn cancelled_question_call_does_not_reach_next_request() {
    let body = request_body_with(vec![
        EventKind::TurnStart { turn: 1 },
        EventKind::StepStart { turn: 1, step: 1 },
        assistant_with_call("我先问一句", "orphan-call"),
        EventKind::QuestionAsked {
            turn: 1,
            step: 1,
            call_id: "orphan-call".to_owned(),
            question_id: "q1".to_owned(),
            question: "要不要继续？".to_owned(),
            header: None,
            options: vec!["要".to_owned()],
            multi_select: false,
        },
        EventKind::QuestionCancelled {
            turn: 1,
            step: 1,
            call_id: "orphan-call".to_owned(),
            question_id: "q1".to_owned(),
        },
        EventKind::StepEnd { turn: 1, step: 1 },
        EventKind::TurnEnd {
            turn: 1,
            reason: session::TurnEndReason::Completed,
        },
    ])
    .await;
    assert!(!body.contains("orphan-call"), "孤儿 tool call 不能进请求体");
    assert!(
        !body.contains("question/cancelled"),
        "取消事件不是消息，不能出现在请求体"
    );
}

#[tokio::test]
async fn paired_tool_call_and_result_stay() {
    let mut assistant = assistant_with_call("我先读一下", "paired-call");
    if let EventKind::AssistantMessage { message, .. } = &mut assistant {
        message.content.retain(|block| {
            matches!(block, ContentBlock::ToolCall { .. }) || block.as_text().is_some()
        });
    }
    let body = request_body_with(vec![
        EventKind::TurnStart { turn: 1 },
        EventKind::StepStart { turn: 1, step: 1 },
        assistant,
        EventKind::ToolResult {
            turn: 1,
            step: 1,
            message: Message::tool_result("paired-call", "结果", false),
            error: None,
            meta: None,
        },
        EventKind::StepEnd { turn: 1, step: 1 },
        EventKind::TurnEnd {
            turn: 1,
            reason: session::TurnEndReason::Completed,
        },
    ])
    .await;
    assert!(body.contains("paired-call"), "正常配对必须保留");
    assert!(body.contains("结果"), "正常 tool result 必须保留");
}
