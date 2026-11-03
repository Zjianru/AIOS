use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use model::{CancelToken, CredentialSource, DeepSeek, DeepSeekConfig, ErrorCode, ModelError};

struct FailingSource;

impl CredentialSource for FailingSource {
    fn issue(&self, _purpose: &str) -> Result<String, ModelError> {
        Err(ModelError::new(
            ErrorCode::MissingCredential,
            "Keyring 无法签发：钥匙文件不在",
        ))
    }
}

struct CountingSource {
    calls: AtomicUsize,
}

impl CredentialSource for CountingSource {
    fn issue(&self, _purpose: &str) -> Result<String, ModelError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok("key-from-source".to_owned())
    }
}

fn config() -> DeepSeekConfig {
    DeepSeekConfig {
        base_url: "http://127.0.0.1:1/anthropic".to_owned(),
        api_key: String::new(),
        ..DeepSeekConfig::default()
    }
}

fn cancel() -> CancelToken {
    CancelToken::new(
        Arc::new(std::sync::atomic::AtomicBool::new(false)),
        Arc::new(tokio::sync::Notify::new()),
    )
}

#[test]
fn 来源失败原样返回() {
    let deepseek = DeepSeek::with_credentials(config(), Arc::new(FailingSource)).expect("构造");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let result = runtime.block_on(deepseek.stream(&[], &[], &cancel()));
    let error = result.err().expect("凭证来源失败时应失败");
    assert_eq!(
        error.code,
        ErrorCode::MissingCredential,
        "错误码应来自凭证来源：{error:?}"
    );
    assert!(
        error.message.contains("Keyring"),
        "错误消息应说明 Keyring：{error:?}"
    );
}

#[test]
fn 每次请求都重新申请() {
    let source = Arc::new(CountingSource {
        calls: AtomicUsize::new(0),
    });
    let deepseek = DeepSeek::with_credentials(config(), source.clone()).expect("构造");
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let _ = runtime.block_on(deepseek.stream(&[], &[], &cancel()));
    let _ = runtime.block_on(deepseek.stream(&[], &[], &cancel()));
    assert_eq!(
        source.calls.load(Ordering::Relaxed),
        2,
        "每次 stream 都要向来源申请一次"
    );
}
