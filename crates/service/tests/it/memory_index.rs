//! 对话索引的边界：只收用户自己的话和 Kind 回答；程序送来的消息不进索引；轮次从事件带出来。

use std::time::Duration;

use memory::MemoryStore;
use memory::types::{SearchHit, WriteContext, WriteOp};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

fn user_event(seq: u64, turn: u64, text: &str, rpc: &str) -> session::Event {
    session::Event::new(
        seq,
        seq,
        session::EventKind::UserMessage {
            turn,
            step: 1,
            message: model::Message::user_text_with_request_id(text, rpc),
        },
    )
}

fn assistant_event(seq: u64, turn: u64, text: &str) -> session::Event {
    session::Event::new(
        seq,
        seq,
        session::EventKind::AssistantMessage {
            turn,
            step: 1,
            message: model::Message::assistant(
                vec![model::ContentBlock::text(text)],
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
        },
    )
}

fn conversation_turn(hits: &[SearchHit]) -> Option<u64> {
    hits.iter().find_map(|hit| match hit {
        SearchHit::Conversation { turn, .. } => Some(*turn),
        _ => None,
    })
}

const PROGRAM_MESSAGES: [(&str, &str); 7] = [
    ("中继卡里的查完了摘要", "task-report:task-1:report-1"),
    ("用户让停任务", "task-stop:task-1:attempt-1"),
    ("任务对话里的抄送", "task-cc:input-1"),
    ("启动中断要接续", "restart:boot-1"),
    ("门判定通过了", "gate-verdict:gate-1:report-1"),
    ("上游变了要重核", "upstream-changed:up-1:down-1:report-1"),
    ("约定到点唤醒", "appointment-wake:app-1:run-1"),
];

#[test]
fn conversation_index_skips_program_messages_and_keeps_turn() {
    let dir = tempfile::tempdir().unwrap();
    let runtime = service::memory::MemoryRuntime::open(dir.path()).unwrap();
    let conversation = "00000000-0000-4000-8000-000000000901";
    let mut events = vec![user_event(1, 1, "用户自己的话先给结论", "u-1")];
    let mut seq = 2;
    for (text, rpc) in PROGRAM_MESSAGES {
        events.push(user_event(seq, seq, text, rpc));
        seq += 1;
    }
    events.push(assistant_event(seq, 9, "Kind 的回答正文"));
    for event in &events {
        runtime.index_session_event(conversation, event);
    }
    let store = runtime.store();
    assert_eq!(
        conversation_turn(&store.search("先给结论", 10).unwrap()),
        Some(1),
        "用户自己的话命中，轮次用事件的 turn"
    );
    assert_eq!(
        conversation_turn(&store.search("Kind 的回答", 10).unwrap()),
        Some(9),
        "Kind 回答的轮次要带出来（不是 0）"
    );
    for (text, _) in PROGRAM_MESSAGES {
        let hits = store.search(text, 10).unwrap();
        assert!(
            !hits
                .iter()
                .any(|hit| matches!(hit, SearchHit::Conversation { .. })),
            "程序送来的消息不该进对话索引：{text}"
        );
    }
}

#[test]
fn backfill_uses_the_same_filter_and_turn() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut sessions = session::SessionStore::create(dir.path(), cwd.path()).unwrap();
    let conversation = sessions.id().clone();
    sessions
        .append(&[
            session::EventKind::UserMessage {
                turn: 1,
                step: 1,
                message: model::Message::user_text_with_request_id("之前说过周报格式", "u-1"),
            },
            session::EventKind::UserMessage {
                turn: 2,
                step: 1,
                message: model::Message::user_text_with_request_id(
                    "中继卡里的查完了摘要",
                    "task-report:task-1:report-1",
                ),
            },
            session::EventKind::AssistantMessage {
                turn: 3,
                step: 1,
                message: model::Message::assistant(
                    vec![model::ContentBlock::text("Kind 的回答正文")],
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
            },
        ])
        .unwrap();
    drop(sessions);
    let runtime = service::memory::MemoryRuntime::open(dir.path()).unwrap();
    runtime.backfill(dir.path(), conversation.as_str());
    let store = runtime.store();
    assert_eq!(
        conversation_turn(&store.search("之前说过", 10).unwrap()),
        Some(1),
        "backfill 的轮次也用事件的 turn"
    );
    assert_eq!(
        conversation_turn(&store.search("Kind 的回答", 10).unwrap()),
        Some(3)
    );
    assert!(
        conversation_turn(&store.search("查完了摘要", 10).unwrap()).is_none(),
        "backfill 也不索引程序送来的消息"
    );
}

async fn open_root(
    data: &std::path::Path,
    cwd: &std::path::Path,
    server: &MockServer,
) -> service::RootHost {
    let mut options = crate::common::options(data);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = server.uri();
    options.config = agent::AgentConfig {
        instruction_roots: vec![data.join("agents-home")],
        skill_roots: vec![data.join("skills-home")],
        ..agent::AgentConfig::default()
    };
    service::RootHost::open(options, model::DEFAULT_CONTEXT_WINDOW).await
}

async fn send(root: &service::RootHost, request_id: &str, content: &str) {
    root.dispatch(service::Call::Send(service::SendRequest {
        request_id: request_id.to_owned(),
        content: content.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }))
    .await
    .expect("send");
    for _ in 0..1000 {
        let response = root
            .dispatch(service::Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let service::Response::Status(status) = response else {
            panic!("expected Status");
        };
        if !status.running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("等根会话空闲超时");
}

#[tokio::test]
async fn search_skips_relay_messages_and_reports_turn() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::text_sse("收到")),
        ]))
        .mount(&server)
        .await;
    let root = open_root(data.path(), cwd.path(), &server).await;
    // 用户自己说的一轮（会被索引进对话）。
    send(&root, "index-user", "周报格式是先列风险再列进展").await;
    // 中继卡：程序送来的消息，不该被索引成用户的话。
    send(
        &root,
        "task-report:task-1:report-1",
        r#"{"kind":"task_report","task":{"id":"task-1","title":"查一下归档"},"state":"ready","summary":"中继卡里的查完了摘要"}"#,
    )
    .await;
    let mut store = None;
    for _ in 0..300 {
        let opened = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
        let hits = opened.search("周报格式", 10).unwrap();
        if conversation_turn(&hits).is_some() {
            store = Some(opened);
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let store = store.expect("对话索引要写进库");
    assert_eq!(
        conversation_turn(&store.search("周报格式", 10).unwrap()),
        Some(1),
        "用户自己的话命中，轮次是 1"
    );
    assert!(
        conversation_turn(&store.search("查完了摘要", 10).unwrap()).is_none(),
        "中继卡内容不该被当用户的话索引"
    );
    root.shutdown().await;
}

// 占位：让未用 import 在未来删除时直接报错（本文件只测索引）。
#[allow(dead_code)]
fn _write_op_marker(_: WriteOp, _: WriteContext) {}
