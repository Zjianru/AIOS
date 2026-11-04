use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use model::{
    CancelToken, CredentialContext, CredentialSource, DeepSeek, DeepSeekConfig, ErrorCode,
    ModelError,
};

struct FailingSource;

impl CredentialSource for FailingSource {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Err(ModelError::new(
            ErrorCode::MissingCredential,
            "Keyring 无法签发：钥匙文件不在",
        ))
    }
}

struct CountingSource {
    calls: AtomicUsize,
    last: Mutex<Option<CredentialContext>>,
}

impl CredentialSource for CountingSource {
    fn issue(&self, _purpose: &str, context: &CredentialContext) -> Result<String, ModelError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        *self.last.lock().expect("last context") = Some(context.clone());
        Ok("key-from-source".to_owned())
    }
}

fn config() -> DeepSeekConfig {
    DeepSeekConfig {
        base_url: "http://127.0.0.1:1/anthropic".to_owned(),
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
        last: Mutex::new(None),
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

#[test]
fn 来源收到本次run和轮() {
    let source = Arc::new(CountingSource {
        calls: AtomicUsize::new(0),
        last: Mutex::new(None),
    });
    let deepseek = DeepSeek::with_credentials(config(), source.clone()).expect("构造");
    deepseek.set_issue_context(CredentialContext {
        run: Some("run-1".to_owned()),
        turn: Some(3),
    });
    let runtime = tokio::runtime::Runtime::new().expect("runtime");
    let _ = runtime.block_on(deepseek.stream(&[], &[], &cancel()));
    let last = source.last.lock().expect("last").clone();
    assert_eq!(
        last,
        Some(CredentialContext {
            run: Some("run-1".to_owned()),
            turn: Some(3)
        }),
        "凭证来源必须拿到本次 run/turn"
    );
}
