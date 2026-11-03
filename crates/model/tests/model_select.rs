use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use model::{CancelToken, DeepSeek, DeepSeekConfig};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

#[tokio::test]
async fn 换模型后下一次请求用新模型() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            "data: [DONE]

",
        ))
        .mount(&server)
        .await;
    let config = DeepSeekConfig {
        base_url: server.uri(),
        api_key: "test-key".to_owned(),
        ..DeepSeekConfig::default()
    };
    let deepseek = DeepSeek::new(config).expect("构造");
    deepseek.set_model("deepseek-pro".to_owned());
    let cancel = CancelToken::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    );
    let _ = deepseek.stream(&[], &[], &cancel).await;
    let requests = server.received_requests().await.expect("请求记录");
    assert_eq!(requests.len(), 1, "应发出一次请求");
    let body: serde_json::Value = serde_json::from_slice(&requests[0].body).expect("body json");
    assert_eq!(
        body["model"], "deepseek-pro",
        "换模型后请求体 model 应是新模型：{body}"
    );
}
