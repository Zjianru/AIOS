//! Kind 根会话的记忆注入：产品 RootHost 打开路径 + 假模型请求体。

use std::sync::Arc;
use std::time::Duration;

use memory::MemoryStore;
use memory::types::{WriteContext, WriteOp};
use service::{DeliveryMode, SendRequest};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

#[tokio::test]
async fn kind_root_request_contains_about_you_and_index() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("知道了"),
        )]))
        .mount(&server)
        .await;

    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
    store
        .write(
            WriteOp::Remember {
                title: "回答先给结论".to_owned(),
                summary: "先给结论".to_owned(),
                body: "然后给理由".to_owned(),
                source_kind: "user".to_owned(),
                source_ref: "u-root".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    store
        .write(
            WriteOp::Note {
                title: "GBK CSV".to_owned(),
                summary: "先看编码".to_owned(),
                body: "sources/ 下有 GBK".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: "k-root".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
    drop(store);

    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    options.model.base_url = server.uri();
    options.config = agent::AgentConfig {
        instruction_roots: vec![data.path().join("agents-home")],
        skill_roots: vec![data.path().join("skills-home")],
        ..agent::AgentConfig::default()
    };
    let root = service::open_root_session(options).await.expect("根会话");
    let host = service::ServiceHost::new(root.agent.clone());
    host.send(SendRequest {
        request_id: "memory-root".to_owned(),
        content: "你好".to_owned(),
        delivery: DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    })
    .await
    .unwrap();
    for _ in 0..100 {
        if !server.received_requests().await.unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let requests = server.received_requests().await.unwrap();
    assert!(!requests.is_empty(), "应有 Kind 请求");
    let body = String::from_utf8_lossy(&requests[0].body);
    assert!(body.contains("回答先给结论"), "请求体缺关于你：{body}");
    assert!(body.contains("条目索引"), "请求体缺索引：{body}");
    assert!(body.contains("GBK CSV"));
    let value: serde_json::Value = serde_json::from_str(&body).expect("请求体 JSON");
    let names: Vec<&str> = value["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in [
        "memory_remember",
        "memory_note",
        "memory_search",
        "memory_read",
        "memory_diary",
    ] {
        assert!(names.contains(&name), "缺常驻工具 {name}");
    }
    assert!(
        !names.contains(&"memory_dream_now"),
        "做梦工具要先加载 skill"
    );
    root.agent.idle().await;
    host.shutdown().await;
}

async fn root_with_next_seq() -> (
    wiremock::MockServer,
    tempfile::TempDir,
    tempfile::TempDir,
    service::RootSession,
    u64,
) {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let mut options = crate::common::options(data.path());
    options.cwd = cwd.path().to_path_buf();
    options.model.base_url = server.uri();
    options.config = agent::AgentConfig {
        instruction_roots: vec![data.path().join("agents-home")],
        skill_roots: vec![data.path().join("skills-home")],
        ..agent::AgentConfig::default()
    };
    let root = service::open_root_session(options).await.expect("根会话");
    let log = std::fs::read_to_string(root.agent.log_path()).unwrap_or_default();
    let max = log
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|value| value.get("seq").and_then(serde_json::Value::as_u64))
        .max()
        .unwrap_or(0);
    (server, data, cwd, root, max + 1)
}

/// 根会话日志里已经落盘的事件（挑 seq、核对工具回执用）。
fn events_of(root: &service::RootSession) -> Vec<session::Event> {
    std::fs::read_to_string(root.agent.log_path())
        .unwrap_or_default()
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// 某次工具结果的展示元信息（记忆小卡等）；没有就是 Null。
fn tool_result_meta(root: &service::RootSession, call_id: &str) -> serde_json::Value {
    events_of(root)
        .into_iter()
        .rev()
        .find_map(|event| match event.kind {
            session::EventKind::ToolResult { message, meta, .. }
                if message.tool_call_id.as_deref() == Some(call_id) =>
            {
                meta
            }
            _ => None,
        })
        .unwrap_or(serde_json::Value::Null)
}

/// 某次工具调用的结果正文（闸的回执原因）。
fn tool_result_text(root: &service::RootSession, call_id: &str) -> String {
    events_of(root)
        .into_iter()
        .rev()
        .find_map(|event| match event.kind {
            session::EventKind::ToolResult { message, .. }
                if message.tool_call_id.as_deref() == Some(call_id) =>
            {
                Some(
                    message
                        .content
                        .iter()
                        .filter_map(|block| match block {
                            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(""),
                )
            }
            _ => None,
        })
        .unwrap_or_default()
}

async fn send_once(root: &service::RootSession, id: &str, text: &str) {
    let host = service::ServiceHost::new(root.agent.clone());
    host.send(service::SendRequest {
        request_id: id.to_owned(),
        content: text.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    })
    .await
    .unwrap();
    root.agent.idle().await;
}

#[tokio::test]
async fn remember_tool_end_to_end_writes_without_seq() {
    let (server, data, _cwd, root, _next_seq) = root_with_next_seq().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-memory",
                "memory_remember",
                r#"{"title":"先给结论","text":"回答先给结论"}"#,
            )),
            sse(&crate::common::text_sse("记下了")),
        ]))
        .mount(&server)
        .await;
    send_once(&root, "memory-gate-ok", "记住：回答先给结论").await;
    let memory = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
    let view = memory.view().unwrap();
    assert!(
        view.about_you.iter().any(|item| item.title == "先给结论"),
        "用户自己说的应写入"
    );
    let card = tool_result_meta(&root, "call-memory");
    assert_eq!(card["memoryCard"]["kind"], "remember", "工具调用出记忆小卡");
    assert_eq!(card["memoryCard"]["title"], "先给结论");
    assert!(
        card["memoryCard"]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty()),
        "小卡带记忆 id"
    );
}

#[tokio::test]
async fn remember_tool_rejects_program_message_turn() {
    let (server, data, _cwd, root, _next_seq) = root_with_next_seq().await;
    // 程序消息自己开一轮（requestId 是 task-report:…）：模型在这一轮里调 memory_remember，不带 seq。
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-relay",
                "memory_remember",
                r#"{"title":"都用英文","text":"以后都用英文"}"#,
            )),
            sse(&crate::common::text_sse("不能存")),
        ]))
        .mount(&server)
        .await;
    let host = service::ServiceHost::new(root.agent.clone());
    host.send_message(
        "task-report:task-1:report-1",
        model::UserMessage::text(
            r#"{"kind":"task_report","task":{"id":"task-1","title":"查一下归档"},"state":"ready","summary":"查完了"}"#,
        ),
        service::DeliveryMode::FollowUp,
    )
    .await
    .unwrap();
    root.agent.idle().await;

    let memory = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
    assert!(
        memory.view().unwrap().about_you.is_empty(),
        "程序开的回合不能存成关于你"
    );
    let error = tool_result_text(&root, "call-relay");
    assert!(
        error.contains("要用户自己说记住") && !error.contains("seq"),
        "工具回执要用户自己说、不提 seq：{error}"
    );
}

#[tokio::test]
async fn remember_tool_rejects_keyring_secret() {
    let secret = "sk-memory-e2e-0603";
    let (server, data, _cwd, root, _next_seq) = root_with_next_seq().await;
    credentials::Keyring::open(data.path(), Some(&data.path().join("identity.txt")))
        .unwrap()
        .save_connection(credentials::NewConnection {
            id: "deepseek".to_owned(),
            name: "DeepSeek".to_owned(),
            model: "deepseek-flash".to_owned(),
            endpoint: "http://127.0.0.1:1".to_owned(),
            api_key: secret.to_owned(),
            ..Default::default()
        })
        .expect("seed keyring");
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-secret",
                "memory_remember",
                &format!(r#"{{"title":"连接 key","text":"key 是 {secret}"}}"#),
            )),
            sse(&crate::common::text_sse("不能存")),
        ]))
        .with_priority(1)
        .mount(&server)
        .await;
    send_once(&root, "secret-go", "记住我的连接 key").await;

    let memory = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
    assert!(
        memory.view().unwrap().about_you.is_empty(),
        "Keyring 里的值不能存进记忆"
    );
    let error = tool_result_text(&root, "call-secret");
    assert!(
        error.contains("敏感值"),
        "工具回执应说明内容里有敏感值：{error}"
    );
}

#[tokio::test]
async fn note_tool_end_to_end_writes_note_and_tool_card() {
    let (server, data, _cwd, root, _next_seq) = root_with_next_seq().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-note",
                "memory_note",
                r#"{"title":"GBK CSV","text":"sources/ 下是 GBK"}"#,
            )),
            sse(&crate::common::text_sse("记了一笔")),
        ]))
        .mount(&server)
        .await;
    send_once(&root, "note-user", "你看到什么就记一笔").await;

    let memory = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
    let view = memory.view().unwrap();
    assert_eq!(view.notes.len(), 1, "Kind 的笔记落库");
    assert_eq!(view.notes[0].title, "GBK CSV");
    let card = tool_result_meta(&root, "call-note");
    assert_eq!(card["memoryCard"]["kind"], "note", "工具调用出笔记小卡");
    assert_eq!(card["memoryCard"]["title"], "GBK CSV");
    assert_eq!(
        card["memoryCard"]["id"].as_str(),
        Some(view.notes[0].id.as_str()),
        "卡上的 id 指回这条笔记"
    );
}

#[tokio::test]
async fn search_tool_end_to_end_finds_indexed_conversation() {
    let (server, data, _cwd, root, _next_seq) = root_with_next_seq().await;
    // 对话索引挂在 ServiceHost 的转发上：这条用例用带记忆库的宿主。
    let memory = Arc::new(service::memory::MemoryRuntime::open(data.path()).unwrap());
    let host = service::ServiceHost::new_with_memory(root.agent.clone(), Some(Arc::clone(&memory)));
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("好"),
        )]))
        .mount(&server)
        .await;
    host.send(service::SendRequest {
        request_id: "search-seed".to_owned(),
        content: "周报格式是先列风险再列进展".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    })
    .await
    .unwrap();
    root.agent.idle().await;
    // 索引由转发端异步写：等这条对话真的可搜，再让模型搜。
    let mut indexed = false;
    for _ in 0..300 {
        let hits = memory.store().search("周报格式", 10).unwrap_or_default();
        if hits
            .iter()
            .any(|hit| matches!(hit, memory::types::SearchHit::Conversation { .. }))
        {
            indexed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(indexed, "Kind 对话应被索引");

    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-search",
                "memory_search",
                r#"{"query":"周报格式"}"#,
            )),
            sse(&crate::common::text_sse("你之前说过：先列风险再列进展")),
        ]))
        .with_priority(1)
        .mount(&server)
        .await;
    host.send(service::SendRequest {
        request_id: "search-turn".to_owned(),
        content: "我之前说过周报怎么写？".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
    })
    .await
    .unwrap();
    root.agent.idle().await;

    let text = tool_result_text(&root, "call-search");
    assert!(text.contains("[对话"), "搜索结果带出处：{text}");
    assert!(text.contains("周报格式"), "命中原话：{text}");
    let card = tool_result_meta(&root, "call-search");
    assert_eq!(card["memoryCard"]["kind"], "search", "工具调用出翻记忆小卡");
    assert_eq!(card["memoryCard"]["query"], "周报格式", "小卡记住搜的什么");
    assert_eq!(
        card["memoryCard"]["target"]["kind"], "conversation",
        "搜索卡的出处目标带会话命中"
    );
    assert!(
        card["memoryCard"]["target"]["turn"]
            .as_u64()
            .is_some_and(|turn| turn > 0),
        "出处目标带轮号：{card}"
    );
    host.shutdown().await;
}

// ---- 记忆页（MemoryRead / MemoryAction / memory/changed） ----

fn page_store(data: &std::path::Path) -> MemoryStore {
    MemoryStore::open(data.join("memory.sqlite")).unwrap()
}

fn seed_memory_page(data: &std::path::Path) {
    let store = page_store(data);
    store
        .write(
            WriteOp::Remember {
                title: "回答先给结论".to_owned(),
                summary: String::new(),
                body: "先给结论再给理由".to_owned(),
                source_kind: "user".to_owned(),
                source_ref: "page-u1".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap();
    store
        .write(
            WriteOp::Note {
                title: "GBK CSV".to_owned(),
                summary: "先看编码".to_owned(),
                body: "sources/ 下有 GBK".to_owned(),
                source_kind: "kind".to_owned(),
                source_ref: "page-k1".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
    store
        .write(
            WriteOp::Diary {
                text: "用户喜欢先给结论".to_owned(),
                source_ref: "page-d1".to_owned(),
            },
            &WriteContext::kind(),
        )
        .unwrap();
}

async fn open_page_root(
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

async fn send_page_turn(root: &service::RootHost, id: &str, text: &str) {
    root.dispatch(service::Call::Send(service::SendRequest {
        request_id: id.to_owned(),
        content: text.to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: Vec::new(),
        canvas: None,
        result_card: None,
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
async fn memory_page_reads_view_search_and_card_states() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_memory_page(data.path());
    // 许可卡：攒够了（min_events 0）但没许可 → 启动那次检查发一张 permission 卡。
    page_store(data.path())
        .update_dream_settings(&memory::types::DreamSettingsPatch {
            min_events: Some(0),
            ..Default::default()
        })
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("好"),
        )]))
        .mount(&server)
        .await;
    let root = open_page_root(data.path(), cwd.path(), &server).await;
    // 先让用户说一句会被索引进对话的话。
    send_page_turn(&root, "page-turn", "周报格式是先列风险再列进展").await;

    let response = root
        .dispatch(service::Call::MemoryRead(service::MemoryReadRequest {
            query: Some("周报格式".to_owned()),
            item_id: None,
            refs: vec!["dream_permit".to_owned()],
        }))
        .await
        .expect("memory-read");
    let service::Response::MemoryRead(read) = response else {
        panic!("expected MemoryRead");
    };
    assert_eq!(read.about_you.len(), 1, "关于你一条");
    assert_eq!(read.about_you[0].title, "回答先给结论");
    assert_eq!(read.notes.len(), 1, "Kind 记的一条");
    assert_eq!(read.diary.len(), 1, "日记只读列出来");
    assert!(
        read.search.iter().any(|hit| hit.kind == "conversation"),
        "搜 Kind 对话命中：{:?}",
        read.search
    );
    let card = read
        .cards
        .iter()
        .find(|card| card.reference == "dream_permit")
        .expect("许可卡状态按 ref 读");
    assert_eq!(card.state, "active", "还没允许");
    assert!(read.dream.runs.is_empty(), "还没跑过做梦");
    root.shutdown().await;
}

#[tokio::test]
async fn card_undo_is_recoverable_and_same_original_words_can_be_remembered_again() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_memory_page(data.path());
    let root = open_page_root(data.path(), cwd.path(), &server).await;
    let store = page_store(data.path());
    let item = store.view().unwrap().about_you[0].clone();

    // 小卡的「撤销」走 Delete：回执写明 7 天可恢复，关于你里消失、进隔离区。
    let response = root
        .dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
            action: service::MemoryActionKind::Delete {
                id: item.id.clone(),
            },
        }))
        .await
        .expect("delete");
    let service::Response::MemoryAction(action) = response else {
        panic!("expected MemoryAction");
    };
    assert!(
        action.message.contains("7 天内能恢复"),
        "回执要写可恢复：{}",
        action.message
    );
    let view = store.view().unwrap();
    assert!(view.about_you.is_empty(), "关于你里不再有");
    assert!(
        view.quarantine.iter().any(|entry| entry.id == item.id),
        "进隔离区"
    );

    // 小卡按 ref 读状态：隔离区条目是 deleted（前端据此显示已撤销、收起按钮）。
    let response = root
        .dispatch(service::Call::MemoryRead(service::MemoryReadRequest {
            query: None,
            item_id: None,
            refs: vec![item.id.clone()],
        }))
        .await
        .expect("memory-read");
    let service::Response::MemoryRead(read) = response else {
        panic!("expected MemoryRead");
    };
    assert_eq!(
        read.cards.first().map(|card| card.state.as_str()),
        Some("deleted"),
        "撤销后小卡状态是 deleted"
    );

    // 同一句原话再说一次：隔离区不是彻底删，来源没有墓碑，能再记进去。
    let again = store
        .write(
            WriteOp::Remember {
                title: item.title.clone(),
                summary: item.summary.clone(),
                body: item.body.clone(),
                source_kind: "user".to_owned(),
                source_ref: "page-u2".to_owned(),
            },
            &WriteContext::user(),
        )
        .expect("同一句原话能再记");
    assert!(again.item.is_some());
    let view = store.view().unwrap();
    assert!(
        view.about_you.iter().any(|entry| entry.body == item.body),
        "再记的回到关于你"
    );
    assert!(
        view.quarantine.iter().any(|entry| entry.id == item.id),
        "旧的还在隔离区"
    );

    // 隔离区恢复：旧的那条回关于你。
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::Restore {
            id: item.id.clone(),
        },
    }))
    .await
    .expect("restore");
    assert!(
        page_store(data.path())
            .view()
            .unwrap()
            .about_you
            .iter()
            .any(|entry| entry.id == item.id),
        "恢复回关于你"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn memory_actions_write_and_push_changed_version() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_memory_page(data.path());
    let root = open_page_root(data.path(), cwd.path(), &server).await;
    let mut events = root.subscribe_events();
    let about_id = page_store(data.path()).view().unwrap().about_you[0]
        .id
        .clone();

    // 更正：出新修订。
    let response = root
        .dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
            action: service::MemoryActionKind::Correct {
                id: about_id,
                title: "回答先给结论".to_owned(),
                summary: String::new(),
                body: "先给结论，再给理由和取舍".to_owned(),
                reason: "user".to_owned(),
            },
        }))
        .await
        .expect("correct");
    let service::Response::MemoryAction(action) = response else {
        panic!("expected MemoryAction");
    };
    assert!(action.version > 0);
    assert_eq!(
        page_store(data.path()).view().unwrap().about_you[0].body,
        "先给结论，再给理由和取舍"
    );
    // 变更要推一条 seq=0 的 memory/changed（页面收到重读）。
    let pushed = loop {
        let event = tokio::time::timeout(Duration::from_secs(2), events.recv())
            .await
            .expect("等 memory/changed 超时")
            .expect("事件");
        if event.event_type == "memory/changed" {
            break event;
        }
    };
    assert_eq!(pushed.seq, 0, "合成事件不推进 lastSeq");
    assert_eq!(pushed.data["version"], action.version);

    // 删到隔离区 → 恢复。
    let note_id = page_store(data.path()).view().unwrap().notes[0].id.clone();
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::Delete {
            id: note_id.clone(),
        },
    }))
    .await
    .expect("delete");
    assert_eq!(page_store(data.path()).view().unwrap().quarantine.len(), 1);
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::Restore {
            id: note_id.clone(),
        },
    }))
    .await
    .expect("restore");
    assert_eq!(page_store(data.path()).view().unwrap().notes.len(), 1);

    // 定下来：原笔记标记 + 新的有约束力条目。
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::Pin {
            id: note_id.clone(),
        },
    }))
    .await
    .expect("pin");
    let view = page_store(data.path()).view().unwrap();
    assert!(
        view.entries
            .iter()
            .any(|item| item.binding.as_str() == "binding")
    );
    assert!(
        view.notes
            .iter()
            .any(|item| item.id == note_id && item.status == "pinned")
    );

    // 做梦撤销：造一次成功的做梦改动，再从页面整次撤。
    let store = page_store(data.path());
    let dream_note = store
        .write(
            WriteOp::DreamApply {
                run: memory::types::DreamRunInput {
                    id: "page-run".to_owned(),
                    model: "test".to_owned(),
                    started_at: 1,
                    ended_at: 2,
                    outcome: "done".to_owned(),
                    report_json: "{}".to_owned(),
                    usage_json: "{}".to_owned(),
                },
                changes: vec![memory::types::DreamChangeInput {
                    memory_id: None,
                    title: "做梦笔记".to_owned(),
                    summary: String::new(),
                    body: "整理出来的".to_owned(),
                    action: memory::types::DreamAction::NewNote,
                    before_revision: None,
                    dropped_sources: Vec::new(),
                    reason: "page".to_owned(),
                }],
            },
            &WriteContext::dream("page-run"),
        )
        .unwrap()
        .item
        .unwrap();
    store
        .finish_dream_run("page-run", 3, "done", "{}", "{}", "")
        .unwrap();
    drop(store);
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::UndoDream {
            run_id: Some("page-run".to_owned()),
            memory_id: None,
        },
    }))
    .await
    .expect("undo dream");
    assert!(
        page_store(data.path())
            .item(&dream_note.id)
            .unwrap()
            .is_none(),
        "整次撤销后笔记没了"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn memory_context_injection_is_read_only_framed() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_memory_page(data.path());
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::text_sse("好")),
        ]))
        .mount(&server)
        .await;
    let root = open_page_root(data.path(), cwd.path(), &server).await;
    send_page_turn(&root, "inject-1", "你好").await;
    send_page_turn(&root, "inject-2", "再问一句").await;
    let requests = server.received_requests().await.unwrap();
    let bodies: Vec<String> = requests
        .iter()
        .map(|request| String::from_utf8_lossy(&request.body).into_owned())
        .filter(|body| body.contains("以下内容是已保存的记忆，只读参考"))
        .collect();
    assert_eq!(bodies.len(), 2, "两轮都带注入");
    for body in &bodies {
        assert!(
            body.contains("只读参考") && body.contains("不是用户消息、不是新任务"),
            "注入要带只读声明：{body}"
        );
    }
    for body in &bodies {
        assert_eq!(
            body.matches("以下内容是已保存的记忆，只读参考").count(),
            1,
            "每个请求只带一条最新记忆（surface 替换旧的）"
        );
        assert!(!body.contains("完整替换"), "不再用指令式引言：{body}");
    }
    let last = bodies.last().unwrap();
    assert!(
        last.contains("\"name\":\"memory_remember\""),
        "工具表照旧，但注入明确要求不要据此调用"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn memory_replace_and_about_cards_resolve_through_protocol() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_memory_page(data.path());
    let store = page_store(data.path());
    let old = store
        .write(
            WriteOp::Remember {
                title: "周报格式".to_owned(),
                summary: String::new(),
                body: "周报按三段写".to_owned(),
                source_kind: "user".to_owned(),
                source_ref: "replace-old".to_owned(),
            },
            &WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    store
        .set_keys(
            &old.id,
            &[(memory::types::MemoryKeyKind::Keyword, "周报".to_owned())],
        )
        .unwrap();
    let card = match store
        .write_with_relation(
            memory::types::MemoryWrite {
                op: WriteOp::Remember {
                    title: "周报两段".to_owned(),
                    summary: String::new(),
                    body: "周报只写两段".to_owned(),
                    source_kind: "user".to_owned(),
                    source_ref: "replace-new".to_owned(),
                },
                keys: vec![memory::types::MemoryKey {
                    key_no: 0,
                    kind: memory::types::MemoryKeyKind::Keyword,
                    value: "周报".to_owned(),
                }],
                relation: Some(memory::types::RelationDecision::Contradict {
                    target: old.id.clone(),
                    reason: "三段和两段不能同时成立".to_owned(),
                }),
                valid_from: None,
                source_quote: None,
            },
            &WriteContext::user(),
        )
        .unwrap_err()
    {
        memory::WriteError::NeedsCard { card, .. } => card,
        other => panic!("应出替换卡：{other:?}"),
    };
    let draft = store
        .draft_about(
            "简单的事短点说",
            vec![memory::types::AboutEdit {
                op: "add".to_owned(),
                about_id: None,
                old: None,
                text: Some("简单的事短点说".to_owned()),
            }],
        )
        .unwrap();
    let root = open_page_root(data.path(), cwd.path(), &server).await;

    let response = root
        .dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
            action: service::MemoryActionKind::ResolveReplace {
                card: card.card_ref.clone(),
                replace: true,
            },
        }))
        .await
        .expect("replace");
    let service::Response::MemoryAction(action) = response else {
        panic!("expected MemoryAction");
    };
    assert!(action.message.contains("替换了"), "{}", action.message);
    let old_after = page_store(data.path()).item(&old.id).unwrap().unwrap();
    assert_eq!(old_after.invalid_reason.as_deref(), Some("replaced"));

    let response = root
        .dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
            action: service::MemoryActionKind::ResolveAboutDraft {
                card: draft.card_ref.clone(),
                save: true,
            },
        }))
        .await
        .expect("about");
    let service::Response::MemoryAction(action) = response else {
        panic!("expected MemoryAction");
    };
    assert!(action.message.contains("关于你"), "{}", action.message);
    assert!(
        page_store(data.path())
            .view()
            .unwrap()
            .about_you
            .iter()
            .any(|item| item.body == "简单的事短点说")
    );
    // 点第二次返回现状态，不重复写。
    let before = page_store(data.path()).view().unwrap().about_you.len();
    root.dispatch(service::Call::MemoryAction(service::MemoryActionRequest {
        action: service::MemoryActionKind::ResolveAboutDraft {
            card: draft.card_ref,
            save: true,
        },
    }))
    .await
    .expect("about again");
    assert_eq!(
        page_store(data.path()).view().unwrap().about_you.len(),
        before,
        "点两次不重复写"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn memory_delivery_is_recorded_after_a_successful_turn() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    seed_memory_page(data.path());
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("好"),
        )]))
        .mount(&server)
        .await;
    let root = open_page_root(data.path(), cwd.path(), &server).await;
    send_page_turn(&root, "delivery-turn", "记住：回答先给结论").await;
    let store = page_store(data.path());
    let about = store.view().unwrap().about_you[0].clone();
    let deliveries = store.deliveries(&about.id).unwrap();
    assert!(
        deliveries.iter().any(|(_, _, kind, _)| kind == "sent"),
        "TurnEnd succeeded 后要有 sent 送达账：{deliveries:?}"
    );
    root.shutdown().await;
}

#[tokio::test]
async fn memory_flashback_used_is_recorded_when_memory_reads_the_sent_id() {
    let server = MockServer::start().await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let store = page_store(data.path());
    let old = store
        .write(
            memory::types::WriteOp::Remember {
                title: "旧事".to_owned(),
                summary: String::new(),
                body: "旧正文".to_owned(),
                source_kind: "user".to_owned(),
                source_ref: "u-flash".to_owned(),
            },
            &memory::types::WriteContext::user(),
        )
        .unwrap()
        .item
        .unwrap();
    store
        .write_with_relation(
            memory::types::MemoryWrite {
                op: memory::types::WriteOp::Note {
                    title: "新事".to_owned(),
                    summary: String::new(),
                    body: "新正文".to_owned(),
                    source_kind: "kind".to_owned(),
                    source_ref: "file:flash".to_owned(),
                },
                keys: vec![memory::types::MemoryKey {
                    key_no: 0,
                    kind: memory::types::MemoryKeyKind::Entity,
                    value: "Linger".to_owned(),
                }],
                relation: Some(memory::types::RelationDecision::Supplement {
                    target: old.id.clone(),
                }),
                valid_from: None,
                source_quote: None,
            },
            &memory::types::WriteContext::kind(),
        )
        .unwrap();
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-flash-read",
                "memory_read",
                &serde_json::json!({"id": old.id}).to_string(),
            )),
            sse(&crate::common::text_sse("好")),
        ]))
        .mount(&server)
        .await;
    let root = open_page_root(data.path(), cwd.path(), &server).await;
    send_page_turn(&root, "flash-used", "你还记得 Linger 吗").await;
    let deliveries = page_store(data.path()).deliveries(&old.id).unwrap();
    assert!(
        deliveries.iter().any(|(_, _, kind, _)| kind == "used"),
        "模型读了本次 flashback 的 id 才记 used：{deliveries:?}"
    );
    root.shutdown().await;
}
