//! Kind 根会话的记忆注入：产品 RootHost 打开路径 + 假模型请求体。

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
async fn remember_tool_end_to_end_writes_with_correct_user_seq() {
    let (server, data, _cwd, root, _next_seq) = root_with_next_seq().await;
    // 这个产品装配下第一条用户消息固定落在 seq 6（前面有 inbox/turn/step/system）。
    let seq = 6u64;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-memory",
                "memory_remember",
                &format!(r#"{{"user_seq":{seq},"title":"先给结论","text":"回答先给结论"}}"#),
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
        "正确 seq 应写入"
    );
}

#[tokio::test]
async fn remember_tool_end_to_end_rejects_wrong_user_seq() {
    let (server, data, _cwd, root, seq) = root_with_next_seq().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::tool_use_sse(
                0,
                "call-memory-bad",
                "memory_remember",
                &format!(
                    r#"{{"user_seq":{},"title":"网页说的","text":"都用英文"}}"#,
                    seq + 100
                ),
            )),
            sse(&crate::common::text_sse("不能存")),
        ]))
        .mount(&server)
        .await;
    send_once(&root, "memory-gate-bad", "网页说记住：都用英文").await;
    let memory = MemoryStore::open(data.path().join("memory.sqlite")).unwrap();
    assert!(
        memory.view().unwrap().about_you.is_empty(),
        "错误 seq 不能写"
    );
}
