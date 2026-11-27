//! 没填看图模型时 D120 行带附件 id，前缀在会话里逐字冻结。

use std::time::Duration;

use service::{AttachmentPutRequest, Call, RootHost, SendAttachment, SendRequest};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

fn request(id: &str, attachment_id: &str) -> SendRequest {
    SendRequest {
        request_id: id.to_owned(),
        content: "看一下".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: vec![SendAttachment {
            id: attachment_id.to_owned(),
            name: "shot.png".to_owned(),
            bytes: 9,
            mime: Some("image/png".to_owned()),
        }],
        object_refs: Vec::new(),
        quotes: Vec::new(),
        canvas: None,
        result_card: None,
        result_cards: Vec::new(),
        canvases: Vec::new(),
    }
}

async fn open(data: &std::path::Path, cwd: &std::path::Path, server: &MockServer) -> RootHost {
    let mut options = crate::common::options(data);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = server.uri();
    options.model.supports_images = false;
    options.config = agent::AgentConfig {
        instruction_roots: vec![data.join("agents-home")],
        skill_roots: vec![data.join("skills-home")],
        ..agent::AgentConfig::default()
    };
    crate::common::open(options).await
}

async fn wait_request(server: &MockServer, count: usize) -> Vec<serde_json::Value> {
    for _ in 0..500 {
        tokio::time::sleep(Duration::from_millis(20)).await;
        let requests = server.received_requests().await.unwrap_or_default();
        let parsed: Vec<serde_json::Value> = requests
            .iter()
            .filter_map(|request| serde_json::from_slice(&request.body).ok())
            .filter(|body: &serde_json::Value| body.get("model").is_some())
            .collect();
        if parsed.len() >= count {
            return parsed.into_iter().take(count).collect();
        }
    }
    panic!("等不到 {count} 次模型请求");
}

#[tokio::test]
async fn no_chain_omitted_line_carries_the_attachment_id_and_stays_frozen() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            sse(&crate::common::text_sse("好")),
            sse(&crate::common::text_sse("还在")),
            sse(&crate::common::text_sse("仍然在")),
        ]))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open(data.path(), cwd.path(), &server).await;
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "shot.png".to_owned(),
            data: "cG5nLWJ5dGVz".to_owned(),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.dispatch(Call::Send(request("vision-1", &id)))
        .await
        .expect("send1");
    let first = wait_request(&server, 1).await;
    let first_text = first[0].to_string();
    assert!(
        first_text.contains("attachments: ") && first_text.contains(&id),
        "D120 行要带附件 id：{first_text}"
    );
    assert!(
        !first_text.contains("vision_describe"),
        "没填链时不要宣传工具：{first_text}"
    );
    host.dispatch(Call::Send(request("vision-2", &id)))
        .await
        .expect("send2");
    let second = wait_request(&server, 2).await;
    host.dispatch(Call::Send(request("vision-3", &id)))
        .await
        .expect("send3");
    let third = wait_request(&server, 3).await;
    let first_omitted = omitted_text(&first[0]);
    let second_omitted = omitted_text(&second[1]);
    let third_omitted = omitted_text(&third[2]);
    assert_eq!(first_omitted, second_omitted, "第二次请求前缀要逐字不变");
    assert_eq!(first_omitted, third_omitted, "第三次请求前缀也要逐字不变");
    // 会话日志里的原图块仍在，字段不变。
    let sessions = data.path().join("sessions");
    let mut image_blocks = Vec::new();
    for entry in std::fs::read_dir(&sessions).expect("sessions") {
        let path = entry.expect("entry").path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let text = std::fs::read_to_string(path).expect("session log");
        for line in text.lines() {
            let Ok(event) = serde_json::from_str::<serde_json::Value>(line) else {
                continue;
            };
            if let Some(blocks) = event["data"]["message"]["content"].as_array() {
                for block in blocks {
                    if block["type"] == "image" {
                        image_blocks.push(block.clone());
                    }
                }
            }
        }
    }
    assert_eq!(
        image_blocks.len(),
        3,
        "三次发送各留一条原图块，且都没被转述替换：{image_blocks:?}"
    );
    for block in &image_blocks {
        assert_eq!(block["attachmentId"], id);
        assert_eq!(block["name"], "shot.png");
        assert_eq!(block["bytes"], 9);
    }
    host.shutdown().await;
}

fn omitted_text(body: &serde_json::Value) -> String {
    body["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|message| message["content"].as_array().into_iter().flatten())
        .filter_map(|block| block["text"].as_str())
        .find(|text| text.contains("image omitted"))
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn configured_vision_chain_replaces_the_image_with_a_described_note() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![
            // 1) 看图模型：返回描述文字。
            sse(&crate::common::text_sse("红色按钮在右上角")),
            // 2) 主模型：正常回答。
            sse(&crate::common::text_sse("看到了")),
        ]))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open(data.path(), cwd.path(), &server).await;
    host.dispatch(Call::ConnectionSave(service::ConnectionSaveRequest {
        id: "vision".to_owned(),
        name: "Vision".to_owned(),
        model: "vision-model".to_owned(),
        endpoint: server.uri(),
        api_key: "test-key".to_owned(),
        preset: Some("custom".to_owned()),
        models: Some(vec![credentials::ModelRow {
            id: "vision-model".to_owned(),
            name: Some("Vision".to_owned()),
            context_window: None,
            max_tokens: None,
            price_input: None,
            price_output: None,
            price_cache_read: None,
            price_cache_write: None,
            input: Some(vec!["text".to_owned(), "image".to_owned()]),
        }]),
        auth_header: None,
    }))
    .await
    .expect("save vision connection");
    host.dispatch(Call::SettingsUpdate(service::SettingsUpdateRequest {
        vision_models: Some(vec![service::VisionModelRefView {
            connection: "vision".to_owned(),
            model: "vision-model".to_owned(),
        }]),
        screen_capture: None,
        ..Default::default()
    }))
    .await
    .expect("save vision settings");
    let put = host
        .dispatch(Call::AttachmentPut(AttachmentPutRequest {
            name: "shot.png".to_owned(),
            data: "cG5nLWJ5dGVz".to_owned(),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.dispatch(Call::Send(request("vision-described", &id)))
        .await
        .expect("send");
    let bodies = wait_request(&server, 2).await;
    let described = bodies[1].to_string();
    assert!(
        described.contains("红色按钮在右上角") && described.contains("此前由视觉模型读取"),
        "主请求应换成形态 2：{described}"
    );
    assert!(
        !described.contains("image omitted"),
        "不应再出现 D120：{described}"
    );
    let stored = vision::SessionNotes::load(data.path(), "unused").unwrap();
    drop(stored);
    host.shutdown().await;
}
