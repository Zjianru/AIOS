//! replay 集成测试共用工具：测试凭证。

use std::sync::Arc;

use model::{CredentialContext, CredentialSource, ModelError};

/// 测试凭证来源：固定返回 test-key。
pub struct TestCredential;

impl CredentialSource for TestCredential {
    fn issue(&self, _purpose: &str, _context: &CredentialContext) -> Result<String, ModelError> {
        Ok("test-key".to_owned())
    }
}

pub fn test_credential() -> Arc<dyn CredentialSource> {
    Arc::new(TestCredential)
}
