//! T100.1：看图模型链的设置读写、校验、协议 36 与 G1 静态通知。

use std::time::Duration;

use service::{
    Call, ConnectionSaveRequest, ModelSettingsRequest, RootHost, SendAttachment, SendRequest,
    SettingsUpdateRequest, VisionModelRefView,
};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn sse(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body.to_owned())
}

async fn open(data: &std::path::Path, cwd: &std::path::Path, server: &MockServer) -> RootHost {
    let mut options = crate::common::options(data);
    options.cwd = cwd.to_path_buf();
    options.model.base_url = server.uri();
    options.config = agent::AgentConfig {
        instruction_roots: vec![data.join("agents-home")],
        skill_roots: vec![data.join("skills-home")],
        ..agent::AgentConfig::default()
    };
    crate::common::open(options).await
}

async fn add_vision_connection(host: &RootHost, id: &str, model: &str, endpoint: &str) {
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: id.to_owned(),
        name: id.to_owned(),
        model: model.to_owned(),
        endpoint: endpoint.to_owned(),
        api_key: "test-key".to_owned(),
        preset: Some("custom".to_owned()),
        models: Some(vec![credentials::ModelRow {
            id: model.to_owned(),
            name: Some(model.to_owned()),
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
}

async fn add_text_connection(host: &RootHost, id: &str, model: &str, endpoint: &str) {
    host.dispatch(Call::ConnectionSave(ConnectionSaveRequest {
        id: id.to_owned(),
        name: id.to_owned(),
        model: model.to_owned(),
        endpoint: endpoint.to_owned(),
        api_key: "test-key".to_owned(),
        preset: Some("custom".to_owned()),
        models: Some(vec![credentials::ModelRow {
            id: model.to_owned(),
            name: Some(model.to_owned()),
            context_window: None,
            max_tokens: None,
            price_input: None,
            price_output: None,
            price_cache_read: None,
            price_cache_write: None,
            input: Some(vec!["text".to_owned()]),
        }]),
        auth_header: None,
    }))
    .await
    .expect("save text connection");
}

#[tokio::test]
async fn vision_chain_round_trips_order_and_rejects_unimage_models() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"data\":[]}"))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open(data.path(), cwd.path(), &server).await;
    add_vision_connection(&host, "v1", "vision-a", &server.uri()).await;
    add_vision_connection(&host, "v2", "vision-b", &server.uri()).await;
    let response = host
        .dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
            vision_models: Some(vec![
                VisionModelRefView {
                    connection: "v2".to_owned(),
                    model: "vision-b".to_owned(),
                },
                VisionModelRefView {
                    connection: "v1".to_owned(),
                    model: "vision-a".to_owned(),
                },
            ]),
            screen_capture: Some(true),
            ..Default::default()
        }))
        .await
        .expect("save chain");
    let service::Response::SettingsUpdate(response) = response else {
        panic!("不是 SettingsUpdate");
    };
    assert_eq!(response.vision_models[0].connection, "v2");
    assert!(response.screen_capture);
    let settings = host
        .dispatch(Call::ModelSettings(ModelSettingsRequest {}))
        .await
        .expect("settings");
    let service::Response::ModelSettings(settings) = settings else {
        panic!("不是 ModelSettings");
    };
    assert_eq!(settings.vision_models.len(), 2);
    assert_eq!(settings.vision_models[0].model, "vision-b");
    assert!(settings.screen_capture);
    // 纯非法 / 混合非法都拒；空表合法。
    add_text_connection(&host, "vtext", "text-only", &server.uri()).await;
    let pure_invalid = host
        .dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
            vision_models: Some(vec![VisionModelRefView {
                connection: "vtext".to_owned(),
                model: "text-only".to_owned(),
            }]),
            screen_capture: None,
            ..Default::default()
        }))
        .await;
    assert!(pure_invalid.is_err(), "只认文字的模型不能进看图链");
    let mixed_invalid = host
        .dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
            vision_models: Some(vec![
                VisionModelRefView {
                    connection: "v1".to_owned(),
                    model: "vision-a".to_owned(),
                },
                VisionModelRefView {
                    connection: "vtext".to_owned(),
                    model: "text-only".to_owned(),
                },
            ]),
            screen_capture: None,
            ..Default::default()
        }))
        .await;
    assert!(mixed_invalid.is_err(), "混合里有非法项也整单拒");
    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        vision_models: Some(Vec::new()),
        screen_capture: None,
        ..Default::default()
    }))
    .await
    .expect("clear chain");
}

#[tokio::test]
async fn g1_filling_the_chain_appends_one_static_notice() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .respond_with(crate::common::SequenceResponder::new(vec![sse(
            &crate::common::text_sse("好"),
        )]))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("{\"data\":[]}"))
        .mount(&server)
        .await;
    let data = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let host = open(data.path(), cwd.path(), &server).await;
    add_vision_connection(&host, "v1", "vision-a", &server.uri()).await;
    let put = host
        .dispatch(Call::AttachmentPut(service::AttachmentPutRequest {
            name: "shot.png".to_owned(),
            data: "cG5nLWJ5dGVz".to_owned(),
        }))
        .await
        .expect("put");
    let id = serde_json::to_value(&put).unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    host.dispatch(Call::Send(SendRequest {
        request_id: "g1-1".to_owned(),
        content: "看一下".to_owned(),
        delivery: service::DeliveryMode::FollowUp,
        pastes: Vec::new(),
        attachments: vec![SendAttachment {
            id,
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
    }))
    .await
    .expect("send");
    tokio::time::sleep(Duration::from_millis(100)).await;
    // 先等前一轮收尾，再填链。
    for _ in 0..300 {
        let status = host
            .dispatch(Call::Status(service::StatusRequest {}))
            .await
            .expect("status");
        let status = serde_json::to_value(&status).unwrap();
        if status["object"]["running"] == serde_json::json!(false) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    host.dispatch(Call::SettingsUpdate(SettingsUpdateRequest {
        vision_models: Some(vec![VisionModelRefView {
            connection: "v1".to_owned(),
            model: "vision-a".to_owned(),
        }]),
        screen_capture: None,
        ..Default::default()
    }))
    .await
    .expect("fill chain");
    let events = crate::common::read_root_events(data.path());
    let notices: Vec<String> = events
        .iter()
        .filter_map(|event| match &event.kind {
            session::EventKind::SystemMessage { message, .. } => Some(
                message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        model::ContentBlock::Text { text, .. } => Some(text.clone()),
                        _ => None,
                    })
                    .collect::<String>(),
            ),
            _ => None,
        })
        .filter(|text| text.contains("看图模型已填"))
        .collect();
    assert_eq!(notices.len(), 1, "只在空→有补一次静态通知");
    host.shutdown().await;
}
