use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use model::{CancelToken, DeepSeek, DeepSeekConfig};

use crate::common::test_credential;
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "用例只核请求里的模型名，流的预期错误不参与断言"
    )]
    let _ = deepseek
        .stream(&[], &model::ToolRequest::default(), &cancel)
        .await;
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "用例只核请求里的模型名，流的预期错误不参与断言"
    )]
    let _ = deepseek
        .stream(&[], &model::ToolRequest::default(), &cancel)
        .await;
    deepseek.set_model("deepseek-later".to_owned());
    #[expect(
        clippy::let_underscore_must_use,
        reason = "用例只核请求里的模型名，流的预期错误不参与断言"
    )]
    let _ = deepseek
        .stream(&[], &model::ToolRequest::default(), &cancel)
        .await;
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
    #[expect(
        clippy::let_underscore_must_use,
        reason = "用例只核请求里的模型名，流的预期错误不参与断言"
    )]
    let _ = deepseek
        .stream(&[], &model::ToolRequest::default(), &cancel)
        .await;
    let requests = server.received_requests().await.expect("请求记录");
    let body: serde_json::Value = serde_json::from_slice(&requests[2].body).expect("body json");
    assert_eq!(body["model"], "deepseek-later", "下一轮才用新模型");
}

/// 本轮冻结的模型是只读事实：begin_turn 后固定，set_model 只改下一轮；end_turn 清掉。
#[test]
fn frozen_model_is_set_at_begin_turn_and_cleared_at_end() {
    let deepseek =
        DeepSeek::with_credentials(DeepSeekConfig::default(), test_credential()).expect("构造");
    assert_eq!(deepseek.frozen_model(), None, "开始一轮前没有冻结");
    deepseek.begin_turn();
    assert_eq!(
        deepseek.frozen_model().as_deref(),
        Some("deepseek-flash"),
        "按 config 的模型冻结"
    );
    deepseek.set_model("deepseek-pro".to_owned());
    assert_eq!(
        deepseek.frozen_model().as_deref(),
        Some("deepseek-flash"),
        "中途换模型不动本轮冻结"
    );
    deepseek.end_turn();
    assert_eq!(deepseek.frozen_model(), None, "收尾后不再是「本轮」");
}
