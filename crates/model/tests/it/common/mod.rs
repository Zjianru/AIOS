//! model 集成测试共用工具：测试凭证与顺序回放。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use model::{CredentialContext, CredentialSource, ModelError};
use wiremock::{Request, Respond, ResponseTemplate};

/// 测试凭证来源：固定返回 test-key（产品不再有固定 key 构造入口）。
pub struct TestCredential;

impl CredentialSource for TestCredential {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("test-key".to_owned())
    }
}

pub fn test_credential() -> Arc<dyn CredentialSource> {
    Arc::new(TestCredential)
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
