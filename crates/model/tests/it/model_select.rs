use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use model::{
    CancelToken, CredentialContext, CredentialSource, DeepSeek, DeepSeekConfig, ModelError,
};

/// T008.F9：测试凭证来源（产品不再有固定 key 构造入口，测试显式注入）。
struct TestCredential;

impl CredentialSource for TestCredential {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("test-key".to_owned())
    }
}

fn test_credential() -> Arc<dyn CredentialSource> {
    Arc::new(TestCredential)
}
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
        ..DeepSeekConfig::default()
    };
    let deepseek = DeepSeek::with_credentials(config, test_credential()).expect("构造");
    deepseek.set_model("deepseek-pro".to_owned());
    deepseek.begin_turn();
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

#[tokio::test]
async fn 本轮冻结模型中途换模型下一轮才生效() {
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
        ..DeepSeekConfig::default()
    };
    let deepseek = DeepSeek::with_credentials(config, test_credential()).expect("构造");
    let cancel = CancelToken::new(
        Arc::new(AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    );
    deepseek.set_model("deepseek-pro".to_owned());
    deepseek.begin_turn();
    let _ = deepseek.stream(&[], &[], &cancel).await;
    deepseek.set_model("deepseek-later".to_owned());
    let _ = deepseek.stream(&[], &[], &cancel).await;
    let requests = server.received_requests().await.expect("请求记录");
    let models: Vec<String> = requests
        .iter()
        .map(|request| {
            serde_json::from_slice::<serde_json::Value>(&request.body).expect("body json")["model"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect();
    assert_eq!(models[0], "deepseek-pro", "本轮先用冻结模型");
    assert_eq!(models[1], "deepseek-pro", "中途换模型不影响本轮");
    deepseek.begin_turn();
    let _ = deepseek.stream(&[], &[], &cancel).await;
    let requests = server.received_requests().await.expect("请求记录");
    let body: serde_json::Value = serde_json::from_slice(&requests[2].body).expect("body json");
    assert_eq!(body["model"], "deepseek-later", "下一轮才用新模型");
}
