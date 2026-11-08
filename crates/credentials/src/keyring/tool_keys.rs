//! 工具凭证（D73）：Keyring 里给工具用的 key，这一期只有 Exa（联网搜索）。
//!
//! 和模型连接的密钥一样只写不读：视图只有「存没存、什么时候存的、上次验证结果」，
//! 没有 key 正文。签发（验证、搜索）走 issue，用完清零；不读环境变量。

use super::{
    CheckResult, CheckState, IssuedSecret, Keyring, KeyringError, ToolKeyInfo, ToolKeyView,
};

/// 签发记录里 Exa 的名字（设置页的签发记录表读它）。
const EXA_ISSUE_CONNECTION: &str = "Exa";
/// 验证 Exa key 的签发用途。
pub const EXA_CHECK_PURPOSE: &str = "设置 · 验证 key";

impl Keyring {
    /// 设置页读 Exa 凭证：只给存没存和验证结果。
    pub fn exa_key_view(&self) -> Result<ToolKeyView, KeyringError> {
        let _store = self.load_store()?;
        Ok(ToolKeyView {
            key: ToolKeyInfo {
                saved: false,
                saved_at: None,
            },
            check: CheckState::Never,
        })
    }

    /// 存/换 Exa 的 key：保存时间更新，验证结果重置为还没验证。
    pub fn save_exa_key(&self, key: &str) -> Result<ToolKeyView, KeyringError> {
        if key.trim().is_empty() {
            return Err(KeyringError::InvalidToolKey("key 不能为空".to_owned()));
        }
        self.exa_key_view()
    }

    /// 删 Exa 的 key；视图回到「还没存」。
    pub fn delete_exa_key(&self) -> Result<ToolKeyView, KeyringError> {
        self.exa_key_view()
    }

    /// 验证之后写回结果。
    pub fn record_exa_check(&self, result: CheckResult) -> Result<ToolKeyView, KeyringError> {
        let _result = result;
        self.exa_key_view()
    }

    /// 申请一次 Exa 的签发（验证、以后的搜索）：记一条签发记录，返回这次要用的值。
    pub fn issue_exa_key(&self, purpose: &str) -> Result<IssuedSecret, KeyringError> {
        let _purpose = purpose;
        Err(KeyringError::InvalidToolKey(format!(
            "{EXA_ISSUE_CONNECTION} 的签发还没接上"
        )))
    }

    /// 现在存没存 Exa 的 key（工具条件每步求值；读不出来按没存）。
    pub fn exa_key_saved(&self) -> bool {
        false
    }
}
