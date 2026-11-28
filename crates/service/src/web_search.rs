//! 联网搜索服务商：设置页的读/选/存 key/地址/认证/检测，和工具门的开工条件。
//!
//! 同一时间只选一家；没选或缺 key/地址（智谱/DeepSeek 要模型连接的 key）时 web 整组
//! 不开。检测失败只写回检查状态，不挡工具门；key 永不回显。

use crate::ServiceError;
use crate::protocol::{
    WebSearchAuthSetRequest, WebSearchHostSetRequest, WebSearchKeyRequest, WebSearchKeySaveRequest,
    WebSearchProviderView, WebSearchReadResponse, WebSearchSelectRequest,
};
use crate::root_lifecycle::RootHost;
use crate::settings_surface::now_seconds;
use crate::web::providers::{self, KeyNeed, ProviderSpec, SearchError};
use credentials::CheckResult;

/// 页面上每家服务商的 key 需求文案。
fn key_need_label(need: KeyNeed) -> &'static str {
    match need {
        KeyNeed::Required => "required",
        KeyNeed::Optional => "optional",
        KeyNeed::None => "none",
        KeyNeed::Llm => "llm",
    }
}

/// 选中那家的开工条件；None = 没选。
#[derive(Debug, Clone)]
pub(crate) struct SelectedSearch {
    pub(crate) spec: &'static ProviderSpec,
    pub(crate) host: String,
    pub(crate) key: Option<String>,
    pub(crate) basic: Option<(String, String)>,
    pub(crate) missing: Option<String>,
}

impl RootHost {
    fn web_settings(&self) -> crate::settings::Settings {
        crate::settings::load(&self.options.data_dir)
    }

    fn save_web_settings(&self, settings: &crate::settings::Settings) -> Result<(), ServiceError> {
        crate::settings::save(&self.options.data_dir, settings)
            .map_err(ServiceError::SettingsInvalid)
    }

    fn spec(&self, id: &str) -> Result<&'static ProviderSpec, ServiceError> {
        providers::provider(id)
            .ok_or_else(|| ServiceError::SettingsInvalid(format!("没有这个搜索服务商：{id}")))
    }

    fn llm_connection(
        &self,
        keyring: &credentials::Keyring,
        spec: &ProviderSpec,
    ) -> Option<String> {
        let ids = keyring.view().connections;
        let wanted = match spec.id {
            "zhipu" => "zhipu",
            "deepseek" => "deepseek",
            _ => return None,
        };
        let synced = keyring.search_sync_connection(spec.id).ok().flatten();
        if let Some(id) = synced {
            return ids
                .iter()
                .find(|connection| connection.id == id && connection.key_saved)
                .map(|connection| connection.id.clone());
        }
        ids.iter()
            .find(|connection| connection.preset == wanted && connection.key_saved)
            .map(|connection| connection.id.clone())
    }

    fn key_saved(&self, keyring: &credentials::Keyring, spec: &ProviderSpec) -> bool {
        match spec.key_need {
            KeyNeed::Required | KeyNeed::Optional => keyring.search_key_saved(spec.id),
            KeyNeed::None => true,
            KeyNeed::Llm => self.llm_connection(keyring, spec).is_some(),
        }
    }

    /// 选中的服务商 + 地址/凭据；工具门和检测共用这一份判断。
    pub(crate) fn selected_search(&self) -> Result<Option<SelectedSearch>, ServiceError> {
        let settings = self.web_settings();
        let Some(id) = settings.web_search.selected.as_deref() else {
            return Ok(None);
        };
        let spec = self.spec(id)?;
        let keyring = self.keyring()?;
        let host = settings
            .web_search
            .hosts
            .get(spec.id)
            .map(String::as_str)
            .map(|value| providers::host(spec, Some(value)))
            .unwrap_or_else(|| providers::host(spec, None));
        let mut missing = None;
        if host.trim().is_empty() {
            missing = Some("地址还没填".to_owned());
        }
        let key = match spec.key_need {
            KeyNeed::Required => {
                if keyring.search_key_saved(spec.id) {
                    Some(keyring.issue_search_key(spec.id, "网页搜索")?.into_secret())
                } else {
                    missing = Some("还没有 key".to_owned());
                    None
                }
            }
            KeyNeed::Optional => {
                if keyring.search_key_saved(spec.id) {
                    Some(keyring.issue_search_key(spec.id, "网页搜索")?.into_secret())
                } else {
                    None
                }
            }
            KeyNeed::None => None,
            KeyNeed::Llm => {
                let Some(connection) = self.llm_connection(&keyring, spec) else {
                    missing = Some("模型连接里没有可用的 key".to_owned());
                    return Ok(Some(SelectedSearch {
                        spec,
                        host,
                        key: None,
                        basic: None,
                        missing,
                    }));
                };
                Some(
                    keyring
                        .issue(&connection, "网页搜索", None, None)?
                        .into_secret(),
                )
            }
        };
        let basic = if spec.basic_auth && keyring.search_basic_saved() {
            let user = settings
                .web_search
                .auth
                .get(spec.id)
                .map(|auth| auth.user.clone())
                .unwrap_or_default();
            Some((user, keyring.issue_search_basic("网页搜索")?.into_secret()))
        } else {
            None
        };
        Ok(Some(SelectedSearch {
            spec,
            host,
            key,
            basic,
            missing,
        }))
    }

    /// 工具门：选中那家能不能开工；读不出/没选都算不开。
    pub(crate) fn web_search_ready(&self) -> bool {
        self.selected_search()
            .ok()
            .flatten()
            .is_some_and(|selected| selected.missing.is_none())
    }

    /// 设置页读状态（不含 key 正文）。
    pub(crate) async fn web_search_read(&self) -> Result<WebSearchReadResponse, ServiceError> {
        let settings = self.web_settings();
        let keyring = self.keyring()?;
        let basic = keyring.search_basic_view().ok();
        let providers = providers::PROVIDERS
            .iter()
            .map(|spec| {
                let view = keyring
                    .search_key_view(spec.id)
                    .unwrap_or(credentials::ToolKeyView {
                        key: credentials::ToolKeyInfo {
                            saved: false,
                            saved_at: None,
                        },
                        check: credentials::CheckState::Never,
                    });
                let saved = self.key_saved(&keyring, spec);
                let saved_at = if saved {
                    view.key
                        .saved_at
                        .or_else(|| self.llm_connection(&keyring, spec).map(|_| now_seconds()))
                } else {
                    None
                };
                let auth_user = settings
                    .web_search
                    .auth
                    .get(spec.id)
                    .map(|auth| auth.user.clone())
                    .unwrap_or_default();
                WebSearchProviderView {
                    id: spec.id.to_owned(),
                    name: spec.name.to_owned(),
                    key_need: key_need_label(spec.key_need).to_owned(),
                    key_site: spec.key_site.to_owned(),
                    site: spec.site.to_owned(),
                    default_host: spec.host.to_owned(),
                    host: settings
                        .web_search
                        .hosts
                        .get(spec.id)
                        .cloned()
                        .unwrap_or_else(|| spec.host.to_owned()),
                    key_saved: saved,
                    key_saved_at: saved_at,
                    basic_supported: spec.basic_auth,
                    basic_user: auth_user,
                    basic_saved: spec.basic_auth
                        && basic.as_ref().is_some_and(|view| view.key.saved),
                    check: view.check,
                }
            })
            .collect();
        Ok(WebSearchReadResponse {
            selected: settings
                .web_search
                .selected
                .clone()
                .filter(|id| providers::provider(id).is_some()),
            providers,
        })
    }

    async fn after_web_change(&self) -> Result<(), ServiceError> {
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(())
    }

    pub(crate) async fn web_search_select(
        &self,
        request: WebSearchSelectRequest,
    ) -> Result<WebSearchReadResponse, ServiceError> {
        let selected = request.id.filter(|id| !id.trim().is_empty());
        if let Some(id) = &selected {
            self.spec(id)?;
        }
        let mut settings = crate::settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        settings.web_search.selected = selected;
        self.save_web_settings(&settings)?;
        self.after_web_change().await?;
        self.web_search_read().await
    }

    pub(crate) async fn web_search_key_save(
        &self,
        request: WebSearchKeySaveRequest,
    ) -> Result<WebSearchReadResponse, ServiceError> {
        let spec = self.spec(&request.id)?;
        if spec.key_need == KeyNeed::Llm {
            return Err(ServiceError::SettingsInvalid(
                "这家服务商使用模型连接的 key，不在这里单独存".to_owned(),
            ));
        }
        self.keyring()?.save_search_key(spec.id, &request.key)?;
        if spec.id == "deepseek" {
            self.keyring()?
                .set_search_sync_connection("deepseek", Some("deepseek"))?;
        }
        self.after_web_change().await?;
        self.web_search_read().await
    }

    pub(crate) async fn web_search_key_delete(
        &self,
        request: WebSearchKeyRequest,
    ) -> Result<WebSearchReadResponse, ServiceError> {
        let spec = self.spec(&request.id)?;
        self.keyring()?.delete_search_key(spec.id)?;
        self.after_web_change().await?;
        self.web_search_read().await
    }

    pub(crate) async fn web_search_host_set(
        &self,
        request: WebSearchHostSetRequest,
    ) -> Result<WebSearchReadResponse, ServiceError> {
        let spec = self.spec(&request.id)?;
        let host = request.host.trim().trim_end_matches('/').to_owned();
        let mut settings = crate::settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        if host.is_empty() || host == spec.host {
            settings.web_search.hosts.remove(spec.id);
        } else {
            settings.web_search.hosts.insert(spec.id.to_owned(), host);
        }
        self.save_web_settings(&settings)?;
        self.after_web_change().await?;
        self.web_search_read().await
    }

    pub(crate) async fn web_search_auth_set(
        &self,
        request: WebSearchAuthSetRequest,
    ) -> Result<WebSearchReadResponse, ServiceError> {
        let spec = self.spec(&request.id)?;
        if !spec.basic_auth {
            return Err(ServiceError::SettingsInvalid(
                "这家服务商不支持 Basic 认证".to_owned(),
            ));
        }
        let mut settings = crate::settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        let mut auth = settings
            .web_search
            .auth
            .get(spec.id)
            .cloned()
            .unwrap_or_default();
        auth.user = request.user.trim().to_owned();
        if auth.user.is_empty() {
            settings.web_search.auth.remove(spec.id);
        } else {
            settings.web_search.auth.insert(spec.id.to_owned(), auth);
        }
        self.save_web_settings(&settings)?;
        if let Some(password) = request.password {
            if password.trim().is_empty() {
                self.keyring()?.delete_search_basic()?;
            } else {
                self.keyring()?.save_search_basic(&password)?;
            }
        }
        self.after_web_change().await?;
        self.web_search_read().await
    }

    pub(crate) async fn web_search_check(&self) -> Result<WebSearchReadResponse, ServiceError> {
        let Some(selected) = self.selected_search()? else {
            return Err(ServiceError::SettingsInvalid(
                "先选一家搜索服务商再检测".to_owned(),
            ));
        };
        let at = now_seconds();
        let result = match &selected.missing {
            Some(reason) => CheckResult::Fail {
                at,
                reason: reason.clone(),
            },
            None => {
                let basic = selected
                    .basic
                    .as_ref()
                    .map(|(user, password)| (user.as_str(), password.as_str()));
                match providers::search_one(
                    selected.spec,
                    &selected.host,
                    selected.key.as_deref(),
                    basic,
                    "AIOS",
                )
                .await
                {
                    Ok(_) => CheckResult::Ok { at },
                    Err(error) => CheckResult::Fail {
                        at,
                        reason: failure_reason(error, selected.spec.name),
                    },
                }
            }
        };
        self.keyring()?
            .record_search_check_any(selected.spec.id, result)?;
        self.after_web_change().await?;
        self.web_search_read().await
    }
}

fn failure_reason(error: SearchError, name: &str) -> String {
    match error {
        SearchError::Client => "建不出 HTTP 客户端".to_owned(),
        SearchError::Connect => format!("连不上 {name}"),
        SearchError::Timeout => format!("{name} 超时"),
        SearchError::Transport => format!("向 {name} 请求失败"),
        SearchError::Http(status) => format!("{name} 返回 HTTP {status}"),
        SearchError::Body => format!("{name} 返回的内容看不懂"),
        SearchError::Task => "搜索任务没有正常结束".to_owned(),
    }
}

/// 不依赖 RootHost 的选中面：工具门和 web_search 执行都用它。
pub(crate) fn selected_for(
    data_dir: &std::path::Path,
    identity: Option<&std::path::Path>,
) -> Result<Option<SelectedSearch>, ServiceError> {
    let settings = crate::settings::load(data_dir);
    let Some(id) = settings.web_search.selected.as_deref() else {
        return Ok(None);
    };
    let spec = providers::provider(id)
        .ok_or_else(|| ServiceError::SettingsInvalid(format!("没有这个搜索服务商：{id}")))?;
    let keyring = credentials::Keyring::open(data_dir, identity)?;
    let connections = keyring.view().connections;
    let host = settings
        .web_search
        .hosts
        .get(spec.id)
        .map(String::as_str)
        .map(|value| providers::host(spec, Some(value)))
        .unwrap_or_else(|| providers::host(spec, None));
    let mut missing = None;
    if host.trim().is_empty() {
        missing = Some("地址还没填".to_owned());
    }
    let mut llm_connection = || {
        let synced = keyring.search_sync_connection(spec.id).ok().flatten();
        if let Some(synced_id) = synced {
            return connections
                .iter()
                .find(|connection| connection.id == synced_id && connection.key_saved)
                .map(|connection| connection.id.clone());
        }
        let wanted = match spec.id {
            "zhipu" => "zhipu",
            "deepseek" => "deepseek",
            _ => return None,
        };
        connections
            .iter()
            .find(|connection| connection.preset == wanted && connection.key_saved)
            .map(|connection| connection.id.clone())
    };
    let key = match spec.key_need {
        KeyNeed::Required => {
            if keyring.search_key_saved(spec.id) {
                Some(keyring.issue_search_key(spec.id, "网页搜索")?.into_secret())
            } else {
                missing = Some("还没有 key".to_owned());
                None
            }
        }
        KeyNeed::Optional => {
            if keyring.search_key_saved(spec.id) {
                Some(keyring.issue_search_key(spec.id, "网页搜索")?.into_secret())
            } else {
                None
            }
        }
        KeyNeed::None => None,
        KeyNeed::Llm => {
            let Some(connection) = llm_connection() else {
                missing = Some("模型连接里没有可用的 key".to_owned());
                return Ok(Some(SelectedSearch {
                    spec,
                    host,
                    key: None,
                    basic: None,
                    missing,
                }));
            };
            Some(
                keyring
                    .issue(&connection, "网页搜索", None, None)?
                    .into_secret(),
            )
        }
    };
    let basic = if spec.basic_auth && keyring.search_basic_saved() {
        let user = settings
            .web_search
            .auth
            .get(spec.id)
            .map(|auth| auth.user.clone())
            .unwrap_or_default();
        Some((user, keyring.issue_search_basic("网页搜索")?.into_secret()))
    } else {
        None
    };
    Ok(Some(SelectedSearch {
        spec,
        host,
        key,
        basic,
        missing,
    }))
}

/// 工具门用：选中且无缺项。
pub(crate) fn ready_for(data_dir: &std::path::Path, identity: Option<&std::path::Path>) -> bool {
    selected_for(data_dir, identity)
        .ok()
        .flatten()
        .is_some_and(|selected| selected.missing.is_none())
}
