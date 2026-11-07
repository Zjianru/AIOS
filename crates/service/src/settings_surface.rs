//! 设置面：Keyring 连接、模型设置、验证与余额；所有方法经 RootHost 的 Keyring/设置。

use std::path::{Path, PathBuf};

use crate::error::ServiceError;
use crate::protocol::{
    AvailableModelsMode, AvailableModelsRequest, AvailableModelsResponse, ConnectionDeleteRequest,
    ConnectionDeleteResponse, ConnectionQuotaRequest, ConnectionQuotaResponse,
    ConnectionRevokeRequest, ConnectionRevokeResponse, ConnectionSaveRequest,
    ConnectionSaveResponse, ConnectionUpdateRequest, ConnectionUpdateResponse,
    ConnectionVerifyRequest, ConnectionVerifyResponse, FirstUseResponse, IssuesResponse,
    KeyringRelocateRequest, KeyringRelocateResponse, KeyringReopenRequest, KeyringReopenResponse,
    KeyringRestartResponse, ModelSettingsResponse, RootState, ServiceEvent, SettingsUpdateRequest,
    SettingsUpdateResponse,
};
use crate::root_lifecycle::RootHost;
use crate::{quota, settings};

/// 把设置页路径里的 ~ 展开为 home；非绝对路径/未知 ~ 用户明确拒绝。
pub(crate) fn expand_user_path(raw: &str, home: Option<&Path>) -> Result<PathBuf, ServiceError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ServiceError::InvalidPath("路径不能为空".to_owned()));
    }
    let expanded = if raw == "~" || raw.starts_with("~/") {
        let home = home.ok_or_else(|| {
            ServiceError::InvalidPath("要找 home 目录，但当前进程没有 HOME".to_owned())
        })?;
        if raw == "~" {
            home.to_path_buf()
        } else {
            home.join(&raw[2..])
        }
    } else if raw.starts_with('~') {
        return Err(ServiceError::InvalidPath(
            "只支持 ~ 或 ~/ 开头；~user 不支持".to_owned(),
        ));
    } else {
        PathBuf::from(raw)
    };
    if !expanded.is_absolute() {
        return Err(ServiceError::InvalidPath(
            "要写完整路径（/ 或 ~/ 开头）".to_owned(),
        ));
    }
    Ok(expanded)
}

impl RootHost {
    /// 每次设置操作重新打开 Keyring（不缓存；换 key/换位置立即生效）。
    /// Keyring 变化后把当前持有的 key 交给装配层的刷新钩子（监控脱敏用）。
    pub(crate) fn notify_keyring_changed(&self) {
        if let Some(hook) = &self.options.keyring_changed
            && let Ok(keyring) = self.keyring()
        {
            hook(&keyring);
        }
    }

    pub(crate) fn keyring(&self) -> Result<credentials::Keyring, ServiceError> {
        Ok(credentials::Keyring::open(
            &self.options.data_dir,
            self.options.identity.as_deref(),
        )?)
    }

    /// 把模型选择写成同一份事实：settings.json 的 lastSelection + 内存 selection。
    /// 先落盘，写失败不改内存——页面看到错误，不会以为已经保存。
    pub(crate) fn remember_selection(
        &self,
        selection: &session::ModelSelection,
    ) -> Result<(), ServiceError> {
        let mut settings = settings::load(&self.options.data_dir);
        settings.last_selection = Some(selection.clone());
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        *self.selection.lock().expect("selection 锁被毒化") = Some(selection.clone());
        Ok(())
    }

    /// 设置页的 Keyring 视图（响应不含 key 正文）加上屏蔽表和每个连接存下的厂商列表。
    ///
    /// 屏蔽的命中/未知还没有数据来源，先给空值；每个连接存下的厂商列表是真的，页面小标签用它。
    pub async fn model_settings(&self) -> Result<ModelSettingsResponse, ServiceError> {
        let view = self.keyring()?.view();
        let mut listed = std::collections::BTreeMap::new();
        for connection in &view.connections {
            if let Some(live) = model::catalog::read_live(&self.options.data_dir, &connection.id) {
                listed.insert(
                    connection.id.clone(),
                    crate::protocol::ListedModels {
                        fetched_at: live.fetched_at,
                        models: live.models,
                    },
                );
            }
        }
        Ok(ModelSettingsResponse {
            settings: view,
            disabled_models: crate::disabled::disabled_models(&self.options.data_dir),
            disabled_unknown: false,
            disabled_hits: Vec::new(),
            listed,
        })
    }

    pub async fn connection_save(
        &self,
        request: ConnectionSaveRequest,
    ) -> Result<ConnectionSaveResponse, ServiceError> {
        let connection = self
            .keyring()?
            .save_connection(credentials::NewConnection {
                id: request.id,
                name: request.name,
                model: request.model,
                endpoint: request.endpoint,
                api_key: request.api_key,
                preset: request.preset,
                models: request.models,
                price_rate: request.price_rate,
                auth_header: request.auth_header,
            })?;
        // 把刚保存的连接设为当前选择。
        // RootHost 的 recheck 走 model_factory → resolve_selection，只认 CLI 参数 /
        // settings.last_selection / id=="deepseek"；页面/HTTP 建连接（如 minimax-cn）
        // 如果不写 last_selection，recheck 永远报「还没有接模型」（root/unavailable）。
        // 只有「还没接上模型」——RootHost 状态 unavailable——时，
        // 才把刚保存的连接设为当前选择；已接上（Ok）就不换，避免加第二个连接或
        // 重存连接时悄悄改掉用户正在用的模型。
        let set_selection = matches!(
            &*self.state.lock().expect("root state 锁被毒化"),
            RootState::Unavailable { .. }
        );
        let selected = if set_selection {
            let model = crate::models::primary_model(&self.options.data_dir, &connection);
            let provider = model::catalog::provider_id_for_connection(
                &connection.preset,
                &connection.endpoint,
            );
            self.remember_selection(&session::ModelSelection {
                connection: connection.id.clone(),
                // 新会话的档按模型自己的默认档，不是写死的 high。
                effort_wanted: model::default_effort_for(&provider, &connection.endpoint, &model),
                model,
            })
        } else {
            Ok(())
        };
        // 连接本身已经进 Keyring：即使上次选择没写成，也要刷新页面和监控脱敏列表
        // （key 已经换了）；错误放在最后原样报给调用方。
        self.emit_settings_changed();
        self.notify_keyring_changed();
        selected?;
        Ok(ConnectionSaveResponse { connection })
    }

    pub async fn connection_delete(
        &self,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResponse, ServiceError> {
        let deleted = self.keyring()?.delete_connection(&request.id)?;
        if deleted {
            self.emit_settings_changed();
            self.notify_keyring_changed();
        }
        Ok(ConnectionDeleteResponse { deleted })
    }

    /// 编辑连接（只改 endpoint/model，保留 key）。
    ///
    /// 换了厂商身份（地址或预设）就丢掉这个连接存下的厂商列表：那份列表属于旧地址，
    /// 留着会让小标签和主模型继续按旧厂商走，直到下一次获取。
    pub async fn connection_update(
        &self,
        request: ConnectionUpdateRequest,
    ) -> Result<ConnectionUpdateResponse, ServiceError> {
        let keyring = self.keyring()?;
        let before = keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id);
        let connection = keyring.update_connection_details(
            &request.id,
            credentials::ConnectionUpdate {
                endpoint: request.endpoint,
                model: request.model,
                preset: request.preset,
                models: request.models,
                price_rate: request.price_rate,
                auth_header: request.auth_header,
            },
        )?;
        if let Some(before) = before
            && (before.endpoint != connection.endpoint || before.preset != connection.preset)
        {
            model::catalog::remove_live(&self.options.data_dir, &connection.id).map_err(
                |error| ServiceError::SettingsInvalid(format!("旧的模型列表没清掉：{error}")),
            )?;
        }
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(ConnectionUpdateResponse { connection })
    }

    /// 撤销 key（连接保留）。
    pub async fn connection_revoke(
        &self,
        request: ConnectionRevokeRequest,
    ) -> Result<ConnectionRevokeResponse, ServiceError> {
        let connection = self.keyring()?.revoke_key(&request.id)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(ConnectionRevokeResponse { connection })
    }

    /// 验证连接：从 Keyring 取一次签发，发最小 Messages 请求，把结果写进注册器。
    pub async fn connection_verify(
        &self,
        request: ConnectionVerifyRequest,
    ) -> Result<ConnectionVerifyResponse, ServiceError> {
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        let api_key = keyring
            .issue(&request.id, "connection-verify", None, None)?
            .into_secret();
        // 验证走模型适配器（和对话同一条请求路径/同一组头），不再手写 reqwest。
        let config = model::DeepSeekConfig {
            provider: model::catalog::provider_id_for_connection(
                &connection.preset,
                &connection.endpoint,
            ),
            base_url: connection.endpoint.clone(),
            // K3：验证用连接所选模型；跟厂商走时用可选的第一个。
            model: crate::models::primary_model(&self.options.data_dir, &connection),
            max_tokens: 1,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: model::Effort::new("off"),
        };
        let cancel = model::CancelToken::new(
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        let messages = vec![model::Message::user_text("ping")];
        let at = now_seconds();
        // 验证也必须能把学到的鉴权头写回连接，所以把 Keyring 的 Arc 交给来源。
        let keyring = std::sync::Arc::new(keyring);
        let result = match model::DeepSeek::with_credentials(
            config,
            std::sync::Arc::new(VerifyCredential {
                api_key,
                keyring: std::sync::Arc::clone(&keyring),
                connection: request.id.clone(),
                data_dir: self.options.data_dir.clone(),
            }),
        ) {
            Err(error) => credentials::CheckResult::Fail {
                at,
                reason: error.code.as_str().to_owned(),
            },
            Ok(model) => match model.stream(&messages, &[], &cancel).await {
                Ok(_) => credentials::CheckResult::Ok { at },
                Err(error) => credentials::CheckResult::Fail {
                    at,
                    reason: error.code.as_str().to_owned(),
                },
            },
        };
        keyring.record_check(&request.id, result)?;
        self.emit_settings_changed();
        let connection = keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        Ok(ConnectionVerifyResponse { connection })
    }

    /// 目录 + 厂商 GET /v1/models 合并；用户填的字段最后覆盖。
    pub async fn available_models(
        &self,
        request: AvailableModelsRequest,
    ) -> Result<AvailableModelsResponse, ServiceError> {
        // 后面有个同名的局部变量（这次 HTTP 请求），先把意图留下来。
        let mode = request.mode;
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        let key = keyring.issue(&request.id, "models-list", None, None)?;
        let previous = model::catalog::read_live(&self.options.data_dir, &connection.id);
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let provider_id =
            model::catalog::provider_id_for_connection(&connection.preset, &connection.endpoint);
        let catalog_models = catalog.models_for(&provider_id);
        // 「新」的基准：上一次存下的厂商列表；从没存过时内置提供方以目录为基准，
        // 自定义地址没有基准（第一条都不标新）。
        let baseline: Option<std::collections::HashSet<String>> = match previous {
            Some(list) if !list.models.is_empty() => {
                Some(list.models.into_iter().map(|model| model.id).collect())
            }
            _ if provider_id != "custom" => Some(
                catalog_models
                    .iter()
                    .map(|model| model.id.clone())
                    .collect(),
            ),
            _ => None,
        };
        let mut models = catalog_models;
        let mut live = false;
        let mut live_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Ok(client) = model::http::client() {
            // endpoint 以 /anthropic 结尾时列表在 Chat base 的 /v1/models。
            let url = model::catalog::models_url(&connection.endpoint);
            let mut request = client.get(url).timeout(std::time::Duration::from_secs(5));
            match connection.auth_header.as_deref() {
                Some("authorization") => {
                    request =
                        request.header("authorization", format!("Bearer {}", key.into_secret()));
                }
                _ => {
                    request = request.header("x-api-key", key.into_secret());
                }
            }
            if let Ok(response) = request.send().await
                && response.status().is_success()
                && let Ok(bytes) = response.bytes().await
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            {
                let live_models = model::catalog::parse_live_models(&value);
                if !live_models.is_empty() {
                    // 只有用户按了「获取可用模型」才落盘。打开选择器/设置页也会走到这里，
                    // 那一次落盘会把「新」的基准推着走，设置页就再也看不到「新」。
                    // 存的是厂商这次给的列表（没并目录、没写用户字段）；空表不写。
                    if mode == AvailableModelsMode::Fetch {
                        model::catalog::save_live(
                            &self.options.data_dir,
                            &connection.id,
                            now_seconds(),
                            model::catalog::live_models_info(&live_models),
                        )
                        .map_err(|error| {
                            ServiceError::SettingsInvalid(format!("模型列表没保存：{error}"))
                        })?;
                    }
                    live_ids = live_models.iter().map(|model| model.id.clone()).collect();
                    models = model::catalog::merge_live(models, live_models);
                    live = true;
                }
            }
        }
        // 用户填过的字段不动：同名覆盖 name/窗口/输出/用户价格；目录没有的也列出来。
        for row in connection.models.as_deref().unwrap_or_default() {
            if let Some(info) = models.iter_mut().find(|info| info.id == row.id) {
                if let Some(name) = &row.name {
                    info.name = name.clone();
                }
                if row.context_window.is_some() {
                    info.context_window = row.context_window;
                }
                if row.max_tokens.is_some() {
                    info.max_tokens = row.max_tokens;
                }
                if let Some(price) = row.price_input {
                    info.price_input = Some(price);
                    info.price_output.get_or_insert(price);
                }
                if let Some(price) = row.price_output {
                    info.price_output = Some(price);
                }
                if let Some(price) = row.price_cache_read {
                    info.price_cache_read = Some(price);
                }
                if let Some(price) = row.price_cache_write {
                    info.price_cache_write = Some(price);
                }
            } else {
                models.push(model::catalog::ModelInfo {
                    id: row.id.clone(),
                    name: row.name.clone().unwrap_or_else(|| row.id.clone()),
                    context_window: row.context_window,
                    max_tokens: row.max_tokens,
                    price_input: row.price_input,
                    price_output: row.price_output.or(row.price_input),
                    price_cache_read: row.price_cache_read,
                    price_cache_write: row.price_cache_write,
                    thinks: false,
                    efforts: Vec::new(),
                });
            }
        }
        let models = models
            .into_iter()
            .map(|model| {
                // 只有厂商这次给的、上一次没见过的才算「新」；目录独有项/用户行不算。
                let fresh = live
                    && live_ids.contains(&model.id)
                    && baseline
                        .as_ref()
                        .is_some_and(|baseline| !baseline.contains(&model.id));
                crate::protocol::AvailableModelRow { model, fresh }
            })
            .collect();
        Ok(AvailableModelsResponse { models, live })
    }

    pub async fn keyring_relocate(
        &self,
        request: KeyringRelocateRequest,
    ) -> Result<KeyringRelocateResponse, ServiceError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = expand_user_path(&request.to, home.as_deref())?;
        self.keyring()?.relocate_identity(&path)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(KeyringRelocateResponse {})
    }

    pub async fn keyring_reopen(
        &self,
        request: KeyringReopenRequest,
    ) -> Result<KeyringReopenResponse, ServiceError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = expand_user_path(&request.at, home.as_deref())?;
        self.keyring()?.repoint_identity(&path)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(KeyringReopenResponse {})
    }

    pub async fn keyring_restart(&self) -> Result<KeyringRestartResponse, ServiceError> {
        let keyring = self.keyring()?;
        keyring.restart()?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(KeyringRestartResponse {
            settings: keyring.view(),
        })
    }

    pub async fn issues(&self) -> Result<IssuesResponse, ServiceError> {
        let view = self.keyring()?.view();
        Ok(IssuesResponse {
            issued_today: view.issued_today,
            issues: view.issues,
        })
    }

    pub async fn first_use(&self) -> Result<FirstUseResponse, ServiceError> {
        let view = self.keyring()?.view();
        Ok(FirstUseResponse {
            first_use: view.first_use,
            key_file: view.key_file,
        })
    }

    /// 写非密钥设置（目前只有 currency）。只碰 settings.json，不碰 Keyring/连接。
    pub async fn settings_update(
        &self,
        request: SettingsUpdateRequest,
    ) -> Result<SettingsUpdateResponse, ServiceError> {
        let mut settings = settings::load(&self.options.data_dir);
        if let Some(currency) = request.currency {
            match currency.as_str() {
                "usd" | "cny" => settings.currency = currency,
                other => {
                    return Err(ServiceError::SettingsInvalid(format!(
                        "不支持的币种：{other}（只有 usd/cny）"
                    )));
                }
            }
        }
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        Ok(SettingsUpdateResponse {
            currency: settings.currency,
        })
    }

    /// 查询连接的余额/套餐余量（DeepSeek /user/balance、MiniMax /v1/token_plan/remains）。
    /// 查不到不报错（字段为空；spec 故事 5）。
    pub async fn connection_quota(
        &self,
        request: ConnectionQuotaRequest,
    ) -> Result<ConnectionQuotaResponse, ServiceError> {
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        let key = keyring
            .issue(&request.id, "connection-quota", None, None)?
            .into_secret();
        let quota = quota::fetch(&connection, &key).await;
        Ok(ConnectionQuotaResponse {
            balance: quota.balance,
            plan: quota.plan,
            checked_at: now_seconds(),
        })
    }

    /// 设置变化通知（seq=0 的合成事件；页面收到后重新拉设置）。
    pub(crate) fn emit_settings_changed(&self) {
        let event = ServiceEvent {
            seq: 0,
            time: 0,
            event_type: "keyring/changed".to_owned(),
            data: serde_json::json!({}),
            cause: None,
        };
        // 没有订阅端是正常情况（服务刚起/页面没开）；不把广播失败当错误。
        let _delivered = self.events.send(event).unwrap_or(0);
    }
}
/// 验证连接的固定 key 来源（key 只在内存里；签发记录仍由 Keyring 记）。
/// 验证时学到的鉴权头也写回连接（和对话路径同一接缝）。
struct VerifyCredential {
    api_key: String,
    keyring: std::sync::Arc<credentials::Keyring>,
    connection: String,
    /// 验证和聊天共用同一条出站接缝：被屏蔽的模型不发请求。
    data_dir: std::path::PathBuf,
}

impl model::CredentialSource for VerifyCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok(self.api_key.clone())
    }

    /// 连接上记住的鉴权头（第二次验证直接用它，不再从双头开始）。
    fn auth_header(&self) -> Option<String> {
        self.keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == self.connection)
            .and_then(|connection| connection.auth_header)
    }

    /// 验证连接也走屏蔽检查：被屏蔽时记录失败（MODEL_DISABLED），不发出请求。
    fn admit(&self, model: &str) -> Result<(), model::ModelError> {
        crate::disabled::admit(&self.data_dir, &self.connection, model)
    }

    fn remember_auth_header(&self, header: &str) {
        let Some(connection) = self
            .keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == self.connection)
        else {
            return;
        };
        if let Err(error) = self.keyring.update_connection_details(
            &self.connection,
            credentials::ConnectionUpdate {
                endpoint: connection.endpoint.clone(),
                model: connection.primary_model().to_owned(),
                auth_header: Some(header.to_owned()),
                ..credentials::ConnectionUpdate::default()
            },
        ) {
            eprintln!(
                "{}",
                auth_header_writeback_failure(&self.connection, &error.to_string())
            );
        }
    }
}

/// 鉴权头写回 Keyring 失败时的一行可见日志：连接 id + 原因（key 不进日志）。
pub(crate) fn auth_header_writeback_failure(connection: &str, reason: &str) -> String {
    format!("连接 {connection} 的鉴权头没写回 Keyring：{reason}")
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
