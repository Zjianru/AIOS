//! 凭证来源：模型适配器每次出站前向它申请一次 key（T008.2）。
//!
//! 产品实现是 Keyring 的适配器；测试实现可以是固定值或记录器。

use crate::ModelError;

/// 本次出站请求的归属：哪次 run、哪一轮。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CredentialContext {
    pub run: Option<String>,
    pub turn: Option<u64>,
}

/// 出站请求前申请一次凭证。返回值只在这一次请求里用，不缓存。
pub trait CredentialSource: Send + Sync {
    /// purpose 说明这次签发的用途（如 model-request / compaction）；
    /// context 带上本次 Run/轮，供 Keyring 记录归属。
    fn issue(&self, purpose: &str, context: &CredentialContext) -> Result<String, ModelError>;

    /// T012.6：这个连接的鉴权头。`None` = x-api-key 与 Authorization: Bearer 都发（默认）；
    /// `Some("x-api-key")` / `Some("authorization")` = 只发学到的那个（产品层从连接读）。
    fn auth_header(&self) -> Option<String> {
        None
    }

    /// T012.6：适配器学到「只发某一个头」时回报；产品层写回连接的 `auth_header`。
    /// 测试来源可忽略；产品实现是 Keyring。
    fn remember_auth_header(&self, _header: &str) {}
}
