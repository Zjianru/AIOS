//! Kind 根会话的记忆注入：产品 RootHost 打开路径 + 假模型请求体。

use std::time::Duration;

use memory::MemoryStore;
use memory::types::{WriteContext, WriteOp};
use service::{DeliveryMode, SendRequest};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(text: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(crate::common::text_sse(text))
}

#[tokio::test]
async fn kind_root_request_contains_about_you_and_index() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse("知道了")]))
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
