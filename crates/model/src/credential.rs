//! 凭证来源：模型适配器每次出站前向它申请一次 key（T008.2）。
//!
//! 产品实现是 Keyring 的适配器；测试实现可以是固定值或记录器。

use crate::ModelError;

/// 出站请求前申请一次凭证。返回值只在这一次请求里用，不缓存。
pub trait CredentialSource: Send + Sync {
    /// purpose 说明这次签发的用途（如 model-request / compaction）。
    fn issue(&self, purpose: &str) -> Result<String, ModelError>;
}

/// 固定 key 的来源（CLI 旧路径/测试用；T008.2 产品路径换成 Keyring）。
#[derive(Debug)]
pub(crate) struct StaticCredential {
    key: String,
}

impl StaticCredential {
    pub(crate) fn new(key: String) -> Self {
        Self { key }
    }
}

impl CredentialSource for StaticCredential {
    fn issue(&self, _purpose: &str) -> Result<String, ModelError> {
        Ok(self.key.clone())
    }
}
