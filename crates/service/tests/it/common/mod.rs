//! service 集成测试共用工具：测试凭证、根会话 options、顺序回放。

use std::collections::VecDeque;
use std::path::Path;
use std::sync::{Arc, Mutex};

use agent::AgentConfig;
use model::{DEFAULT_CONTEXT_WINDOW, DeepSeekConfig, Effort};
use service::{RootHost, RootSessionOptions};
use wiremock::{Request, Respond, ResponseTemplate};

/// 测试凭证来源：签发固定 test-key；屏蔽判断走产品同一张表。
pub struct TestCredential {
    data_dir: std::path::PathBuf,
    connection: String,
}

impl TestCredential {
    pub fn new(data_dir: &Path, connection: &str) -> Self {
        Self {
            data_dir: data_dir.to_path_buf(),
            connection: connection.to_owned(),
        }
    }
}

impl model::CredentialSource for TestCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok("test-key".to_owned())
    }

    fn admit(&self, model: &str) -> Result<(), model::ModelError> {
        service::disabled::admit(&self.data_dir, &self.connection, model)
    }
}

pub fn test_credential(data_dir: &Path) -> Arc<dyn model::CredentialSource> {
    Arc::new(TestCredential::new(data_dir, "deepseek"))
}

/// 根会话的默认测试配置；调用方按需覆盖字段。
pub fn options(data_dir: &Path) -> RootSessionOptions {
    RootSessionOptions {
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
        model: DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
            tool_update: None,
        },
        config: AgentConfig::default(),
        credentials: Some(test_credential(data_dir)),
        identity: Some(data_dir.join("identity.txt")),
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
    }
}

pub async fn open(options: RootSessionOptions) -> RootHost {
    RootHost::open(options, DEFAULT_CONTEXT_WINDOW).await
}

/// 按脚本顺序回放（第 N 次请求拿第 N 个响应；耗尽回 500，不静默复用）。
pub struct SequenceResponder {
    responses: Mutex<VecDeque<ResponseTemplate>>,
}

impl SequenceResponder {
    pub fn new(responses: Vec<ResponseTemplate>) -> Self {
        Self {
            responses: Mutex::new(responses.into()),
        }
    }
}

impl Respond for SequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        self.responses
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| ResponseTemplate::new(500))
    }
}
