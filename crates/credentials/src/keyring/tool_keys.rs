//! 工具凭证（D73）：Keyring 里给工具用的 key，这一期只有 Exa（联网搜索）。
//!
//! 和模型连接的密钥一样只写不读：视图只有「存没存、什么时候存的、上次验证结果」，
//! 没有 key 正文。签发（验证、搜索）走 issue，用完清零；不读环境变量。

use super::storage::StoredToolKey;
use super::{
    CheckResult, CheckState, IssuedSecret, Keyring, KeyringError, ToolKeyInfo, ToolKeyView,
    now_seconds,
};

/// 签发记录里 Exa 的名字（设置页的签发记录表读它）。
const EXA_ISSUE_CONNECTION: &str = "Exa";
/// 验证 Exa key 的签发用途。
pub const EXA_CHECK_PURPOSE: &str = "设置 · 验证 key";

impl Keyring {
    /// 读一家搜索服务商的凭证视图：只有存没存/保存时间/上次检查，没有 key 正文。
    pub fn search_key_view(&self, provider: &str) -> Result<ToolKeyView, KeyringError> {
        let store = self.load_store()?;
        Ok(view_of(store.tool_keys.get(provider)))
    }

    /// 存/换一家搜索服务商的 key；保存时间更新，检查结果重置。
    pub fn save_search_key(&self, provider: &str, key: &str) -> Result<ToolKeyView, KeyringError> {
        if key.trim().is_empty() {
            return Err(KeyringError::InvalidToolKey("key 不能为空".to_owned()));
        }
        if provider.trim().is_empty() {
            return Err(KeyringError::InvalidToolKey("服务商不能为空".to_owned()));
        }
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        store.tool_keys.insert(
            provider.to_owned(),
            StoredToolKey {
                api_key: key.to_owned(),
                saved_at: now_seconds(),
                check: CheckState::Never,
            },
        );
        self.save_store(&store)?;
        self.notify();
        Ok(view_of(store.tool_keys.get(provider)))
    }

    /// 删一家搜索服务商的 key；视图回到「还没存」。
    pub fn delete_search_key(&self, provider: &str) -> Result<ToolKeyView, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let existed = store.tool_keys.remove(provider).is_some();
        if existed {
            self.save_store(&store)?;
            self.notify();
        }
        Ok(view_of(None))
    }

    /// 验证之后写回一家的检查结果。
    pub fn record_search_check(
        &self,
        provider: &str,
        result: CheckResult,
    ) -> Result<ToolKeyView, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let stored = store.tool_keys.get_mut(provider).ok_or_else(|| {
            KeyringError::InvalidToolKey(format!("还没有 {provider} 的 key"))
        })?;
        stored.check = match result {
            CheckResult::Ok { at } => CheckState::Ok { at },
            CheckResult::Fail { at, reason } => CheckState::Fail { at, reason },
        };
        self.save_store(&store)?;
        self.notify();
        Ok(view_of(store.tool_keys.get(provider)))
    }

    /// 写回一家的检查结果；没有 key 的服务商（DuckDuckGo/SearXNG/LLM）也能留检查状态。
    pub fn record_search_check_any(
        &self,
        provider: &str,
        result: CheckResult,
    ) -> Result<ToolKeyView, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let stored = store
            .tool_keys
            .entry(provider.to_owned())
            .or_insert_with(|| StoredToolKey {
                api_key: String::new(),
                saved_at: 0,
                check: CheckState::Never,
            });
        stored.check = match result {
            CheckResult::Ok { at } => CheckState::Ok { at },
            CheckResult::Fail { at, reason } => CheckState::Fail { at, reason },
        };
        self.save_store(&store)?;
        self.notify();
        Ok(view_of(store.tool_keys.get(provider)))
    }

    /// 申请一次搜索 key 的签发；只记签发记录，返回本次要用的值。
    pub fn issue_search_key(
        &self,
        provider: &str,
        purpose: &str,
    ) -> Result<IssuedSecret, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let secret = store
            .tool_keys
            .get(provider)
            .filter(|stored| !stored.api_key.trim().is_empty())
            .map(|stored| stored.api_key.clone())
            .ok_or_else(|| KeyringError::InvalidToolKey(format!("还没有 {provider} 的 key")))?;
        let display = if provider == "exa" { "Exa" } else { provider };
        store.record_issue(display, purpose, None, None);
        self.save_store(&store)?;
        self.notify();
        Ok(IssuedSecret {
            connection: provider.to_owned(),
            secret,
        })
    }

    /// 这家现在存没存 key（工具门每步求值；读不出来按没存）。
    pub fn search_key_saved(&self, provider: &str) -> bool {
        self.load_store()
            .ok()
            .and_then(|store| store.tool_keys.get(provider).cloned())
            .is_some_and(|stored| !stored.api_key.trim().is_empty())
    }

    /// SearXNG Basic 密码（只写不读；页面只看到存没存）。
    pub fn search_basic_view(&self) -> Result<ToolKeyView, KeyringError> {
        let store = self.load_store()?;
        Ok(view_of(store.search_basic.as_ref()))
    }

    pub fn save_search_basic(&self, password: &str) -> Result<ToolKeyView, KeyringError> {
        if password.trim().is_empty() {
            return Err(KeyringError::InvalidToolKey("密码不能为空".to_owned()));
        }
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        store.search_basic = Some(StoredToolKey {
            api_key: password.to_owned(),
            saved_at: now_seconds(),
            check: CheckState::Never,
        });
        self.save_store(&store)?;
        self.notify();
        Ok(view_of(store.search_basic.as_ref()))
    }

    pub fn delete_search_basic(&self) -> Result<ToolKeyView, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        if store.search_basic.take().is_some() {
            self.save_store(&store)?;
            self.notify();
        }
        Ok(view_of(None))
    }

    pub fn issue_search_basic(&self, purpose: &str) -> Result<IssuedSecret, KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        let secret = store
            .search_basic
            .as_ref()
            .map(|stored| stored.api_key.clone())
            .ok_or_else(|| KeyringError::InvalidToolKey("还没有 SearXNG Basic 密码".to_owned()))?;
        store.record_issue("SearXNG", purpose, None, None);
        self.save_store(&store)?;
        self.notify();
        Ok(IssuedSecret {
            connection: "SearXNG".to_owned(),
            secret,
        })
    }

    pub fn search_basic_saved(&self) -> bool {
        self.load_store()
            .ok()
            .and_then(|store| store.search_basic.map(|stored| !stored.api_key.trim().is_empty()))
            .unwrap_or(false)
    }

    /// DeepSeek 的搜索 key 来源连接 id（Keyring 内部同步；不存副本）。
    pub fn search_sync_connection(&self, provider: &str) -> Result<Option<String>, KeyringError> {
        Ok(self
            .load_store()?
            .search_sync
            .get(provider)
            .cloned())
    }

    pub fn set_search_sync_connection(
        &self,
        provider: &str,
        connection: Option<&str>,
    ) -> Result<(), KeyringError> {
        let _lock = self.lock()?;
        let mut store = self.load_store()?;
        match connection {
            Some(id) => {
                store
                    .search_sync
                    .insert(provider.to_owned(), id.to_owned());
            }
            None => {
                store.search_sync.remove(provider);
            }
        }
        self.save_store(&store)?;
        self.notify();
        Ok(())
    }

    /// 设置页读 Exa 凭证：只给存没存和验证结果。
    pub fn exa_key_view(&self) -> Result<ToolKeyView, KeyringError> {
        self.search_key_view("exa")
    }

    /// 存/换 Exa 的 key。
    pub fn save_exa_key(&self, key: &str) -> Result<ToolKeyView, KeyringError> {
        self.save_search_key("exa", key)
    }

    /// 删 Exa 的 key。
    pub fn delete_exa_key(&self) -> Result<ToolKeyView, KeyringError> {
        self.delete_search_key("exa")
    }

    /// 验证之后写回结果。
    pub fn record_exa_check(&self, result: CheckResult) -> Result<ToolKeyView, KeyringError> {
        self.record_search_check("exa", result)
    }

    /// 申请一次 Exa 的签发。
    pub fn issue_exa_key(&self, purpose: &str) -> Result<IssuedSecret, KeyringError> {
        self.issue_search_key("exa", purpose)
    }

    /// 现在存没存 Exa 的 key。
    pub fn exa_key_saved(&self) -> bool {
        self.search_key_saved("exa")
    }
}

fn view_of(exa: Option<&StoredToolKey>) -> ToolKeyView {
    match exa {
        Some(stored) => ToolKeyView {
            key: ToolKeyInfo {
                saved: true,
                saved_at: Some(stored.saved_at),
            },
            check: stored.check.clone(),
        },
        None => ToolKeyView {
            key: ToolKeyInfo {
                saved: false,
                saved_at: None,
            },
            check: CheckState::Never,
        },
    }
}
