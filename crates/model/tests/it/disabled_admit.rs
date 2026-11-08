//! 出站之前查屏蔽（模型公开边界）：被拒时一个请求都不发，错误码是 `MODEL_DISABLED`。
//!
//! 来源：Magpie 在往上游发请求之前拦（`gateway/disabled.go` 的 `modelOff`）；AIOS 把这一拦
//! 放在适配器的 `stream_inner` 里、签发之前，所以聊天、重试、压缩摘要、验证连接共用一处。

use std::sync::Arc;

use model::{
    CancelToken, CredentialContext, CredentialSource, DeepSeek, DeepSeekConfig, ErrorCode,
    ModelError,
};
use wiremock::matchers::method;
use wiremock::{Mock, MockServer, ResponseTemplate};

/// 放行签发、拒绝出站：被拦时不该走到 issue（走到了也说明拦晚了）。
struct RefusingSource;

impl CredentialSource for RefusingSource {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("test-key".to_owned())
    }

    fn admit(&self, model: &str) -> Result<(), ModelError> {
        Err(ModelError::new(
            ErrorCode::ModelDisabled,
            format!("{model} 已在设置里屏蔽（规则「p」）"),
        ))
    }
}

fn config(server: &MockServer) -> DeepSeekConfig {
    DeepSeekConfig {
        base_url: server.uri(),
        model: "deepseek-flash".to_owned(),
        ..DeepSeekConfig::default()
    }
}

fn cancel() -> CancelToken {
    CancelToken::new(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    )
}

async fn text_server() -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string("data: [DONE]\n\n"),
        )
        .mount(&server)
        .await;
    server
}

async fn request_count(server: &MockServer) -> usize {
    server.received_requests().await.expect("请求记录").len()
}

#[tokio::test]
async fn 被屏蔽的模型一次也不出站() {
    let server = text_server().await;
    let deepseek =
        DeepSeek::with_credentials(config(&server), Arc::new(RefusingSource)).expect("构造");
    let error = deepseek
        .stream(&[], &model::ToolRequest::default(), &cancel())
        .await
        .err()
        .expect("被屏蔽的请求应失败");
    assert_eq!(error.code, ErrorCode::ModelDisabled, "错误码");
    assert!(
        error.message.contains("已在设置里屏蔽"),
        "说明要带屏蔽原因：{error}"
    );
    assert_eq!(
        request_count(&server).await,
        0,
        "被屏蔽的请求一个字节都不发"
    );
}

#[tokio::test]
async fn 压缩摘要的请求同样被拒() {
    let server = text_server().await;
    let deepseek =
        DeepSeek::with_credentials(config(&server), Arc::new(RefusingSource)).expect("构造");
    let error = deepseek
        .stream_with_max_tokens(&[], &model::ToolRequest::default(), &cancel(), Some(256))
        .await
        .err()
        .expect("摘要请求也应失败");
    assert_eq!(error.code, ErrorCode::ModelDisabled);
    assert_eq!(request_count(&server).await, 0, "摘要请求也不出站");
}

/// 守卫：来源放行时照常出站（拦的不是所有请求）。
struct AllowingSource;

impl CredentialSource for AllowingSource {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("test-key".to_owned())
    }
}

#[tokio::test]
async fn 放行的模型照常出站() {
    let server = text_server().await;
    let deepseek =
        DeepSeek::with_credentials(config(&server), Arc::new(AllowingSource)).expect("构造");
    deepseek
        .stream(&[], &model::ToolRequest::default(), &cancel())
        .await
        .expect("放行的请求应成功");
    assert_eq!(request_count(&server).await, 1, "放行的请求照常发出");
}
