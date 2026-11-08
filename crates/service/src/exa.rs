//! Exa（联网搜索）的凭证：设置页的读、存、验证、删，和验证用的最小搜索请求。
//!
//! Keyring 只存 key；这里不读环境变量。验证请求带着 key，所以不跟重定向
//! （避免把授权头带到别的主机），超时 30 秒，和网页工具的上限一致。

use std::time::Duration;

use crate::ServiceError;
use crate::protocol::ExaKeySaveRequest;
use crate::root_lifecycle::RootHost;
use crate::settings_surface::now_seconds;

/// Exa 的默认根地址；设置页的验证和网页搜索都从它拼 /search。
pub const EXA_DEFAULT_BASE_URL: &str = "https://api.exa.ai";
const EXA_TIMEOUT: Duration = Duration::from_secs(30);

impl RootHost {
    /// Exa 的根地址：生产固定，端到端测试可覆盖。
    pub(crate) fn exa_base_url(&self) -> &str {
        self.options
            .exa_base_url
            .as_deref()
            .unwrap_or(EXA_DEFAULT_BASE_URL)
    }

    /// 设置页读 Exa 凭证（不含 key 正文）。
    pub(crate) async fn exa_key(&self) -> Result<credentials::ToolKeyView, ServiceError> {
        Ok(self.keyring()?.exa_key_view()?)
    }

    /// 存/换 Exa 的 key；存完立刻让监控拿到新 key 做脱敏。
    pub(crate) async fn exa_key_save(
        &self,
        request: ExaKeySaveRequest,
    ) -> Result<credentials::ToolKeyView, ServiceError> {
        let view = self.keyring()?.save_exa_key(&request.key)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(view)
    }

    /// 删 Exa 的 key；删掉以后网页工具的条件下一步就不成立。
    pub(crate) async fn exa_key_delete(&self) -> Result<credentials::ToolKeyView, ServiceError> {
        let keyring = self.keyring()?;
        let existed = keyring.exa_key_view()?.key.saved;
        let view = keyring.delete_exa_key()?;
        if existed {
            self.emit_settings_changed();
            self.notify_keyring_changed();
        }
        Ok(view)
    }

    /// 用已存的 key 向 Exa 发一次最小搜索，把结果写回 Keyring。
    pub(crate) async fn exa_key_check(&self) -> Result<credentials::ToolKeyView, ServiceError> {
        let keyring = self.keyring()?;
        if !keyring.exa_key_view()?.key.saved {
            return Err(ServiceError::ExaKeyMissing);
        }
        let key = keyring
            .issue_exa_key(credentials::EXA_CHECK_PURPOSE)?
            .into_secret();
        let at = now_seconds();
        let result = match verify_key(self.exa_base_url(), &key).await {
            Ok(()) => credentials::CheckResult::Ok { at },
            Err(reason) => credentials::CheckResult::Fail { at, reason },
        };
        let view = keyring.record_exa_check(result)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(view)
    }
}

/// 用一把 key 向 Exa 发一次最小搜索。成功返回 Ok；失败只给设置页一句原因，不含 key。
pub async fn verify_key(base_url: &str, api_key: &str) -> Result<(), String> {
    let client = model::http::client_without_redirects()
        .map_err(|_| "出站 HTTP 客户端建不起来".to_owned())?;
    let body = serde_json::json!({
        "query": "AIOS",
        "type": "auto",
        "numResults": 1,
        "contents": {"highlights": {"highlightsPerUrl": 1}}
    })
    .to_string();
    let response = client
        .post(format!("{}/search", base_url.trim_end_matches('/')))
        .timeout(EXA_TIMEOUT)
        .header("authorization", format!("Bearer {api_key}"))
        .header("content-type", "application/json")
        .header("accept", "application/json")
        .body(body)
        .send()
        .await
        .map_err(|error| {
            if error.is_timeout() {
                "等 Exa 回应超时".to_owned()
            } else {
                "连不上 Exa".to_owned()
            }
        })?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status().as_u16();
    if status == 401 || status == 403 {
        return Err(format!("Exa 拒绝了这把 key（{status}）"));
    }
    Err(format!("Exa 那边出错了（{status}）"))
}
