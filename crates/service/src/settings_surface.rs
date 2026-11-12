//! 设置面：Keyring 连接、模型设置、验证与余额；所有方法经 RootHost 的 Keyring/设置。

use std::path::{Path, PathBuf};

use crate::error::ServiceError;
use crate::protocol::{
    AvailableModelsMode, AvailableModelsRequest, AvailableModelsResponse, ConnectionDeleteRequest,
    ConnectionDeleteResponse, ConnectionQuotaRequest, ConnectionQuotaResponse,
    ConnectionRevokeRequest, ConnectionRevokeResponse, ConnectionSaveRequest,
    ConnectionSaveResponse, ConnectionUpdateRequest, ConnectionUpdateResponse,
    ConnectionVerifyRequest, ConnectionVerifyResponse, ConnectionVerifyResult, ExecSettingsView,
    FirstUseResponse, IssuesResponse, KeyringRelocateRequest, KeyringRelocateResponse,
    KeyringReopenRequest, KeyringReopenResponse, KeyringRestartResponse, ModelSettingsResponse,
    RootState, ServiceEvent, SettingsUpdateRequest, SettingsUpdateResponse,
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
        let mut settings = settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        settings.last_selection = Some(selection.clone());
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        *self.selection.lock().expect("selection 锁被毒化") = Some(selection.clone());
        Ok(())
    }

    /// 设置页的 Keyring 视图（响应不含 key 正文）加上屏蔽表、每条规则现在命中的模型，
    /// 和每个连接存下的厂商列表。
    ///
    /// 屏蔽表用带检查的读法：坏文件沿用上次读到的（disabledUnknown 为假），从没读到过
    /// 才是「不知道」（disabledUnknown 为真），不能当成空表。命中表同样从这份表算，
    /// 页面写「现在匹配 n 个」只读它，不自己匹配。
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
        let disabled = crate::disabled::table(&self.options.data_dir);
        let disabled_hits = crate::disabled::hits_by_rule(
            &self.options.data_dir,
            &view.connections,
            &disabled.models,
        );
        Ok(ModelSettingsResponse {
            settings: view,
            disabled_models: disabled.models,
            disabled_unknown: disabled.unknown,
            disabled_hits,
            listed,
        })
    }

    /// 换了厂商身份（地址或预设）就丢掉这个连接存下的厂商列表。
    ///
    /// 那份列表属于旧地址；留着会让小标签和主模型继续按旧厂商走，直到下一次获取。
    fn forget_stale_live(
        &self,
        before: &credentials::ConnectionInfo,
        after: &credentials::ConnectionInfo,
    ) -> Result<(), ServiceError> {
        if before.endpoint == after.endpoint && before.preset == after.preset {
            return Ok(());
        }
        self.remove_live(&after.id)
    }

    /// 删一个连接存下的厂商列表；没存过不算失败。
    fn remove_live(&self, connection: &str) -> Result<(), ServiceError> {
        model::catalog::remove_live(&self.options.data_dir, connection)
            .map_err(|error| ServiceError::SettingsInvalid(format!("旧的模型列表没清掉：{error}")))
    }

    /// 存/换连接。
    ///
    /// 覆盖已有连接且换了地址或预设：旧地址的厂商列表作废，主模型和小标签回到内置目录。
    pub async fn connection_save(
        &self,
        request: ConnectionSaveRequest,
    ) -> Result<ConnectionSaveResponse, ServiceError> {
        let keyring = self.keyring()?;
        let before = keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id);
        let connection = keyring.save_connection(credentials::NewConnection {
            id: request.id,
            name: request.name,
            model: request.model,
            endpoint: request.endpoint,
            api_key: request.api_key,
            preset: request.preset,
            models: request.models,
            auth_header: request.auth_header,
        })?;
        // 覆盖已有连接且换了地址/预设：旧地址的厂商列表作废（同 connection_update）。
        if let Some(before) = before {
            self.forget_stale_live(&before, &connection)?;
        }
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
        // 先删派生出来的厂商列表，再删连接：删列表失败就不删连接，不会留下能被
        // 同 id 重加复活的旧表；删连接本身失败只是丢一份列表，下次获取还能再存。
        self.remove_live(&request.id)?;
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
                auth_header: request.auth_header,
            },
        )?;
        if let Some(before) = before {
            self.forget_stale_live(&before, &connection)?;
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

    /// 验证连接：从 Keyring 取一次签发，发最小 Messages 请求。
    ///
    /// 不带 model 时沿用旧回退，并把结果写进注册器；带 model 时只用它发这一次，
    /// 结果只在响应里返回，不改连接 check、最近选择，也不写适配器学到的鉴权头。
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
        let explicit_model = request.model.is_some();
        let model_id = request
            .model
            .clone()
            .unwrap_or_else(|| crate::models::primary_model(&self.options.data_dir, &connection));
        let config = model::DeepSeekConfig {
            provider: model::catalog::provider_id_for_connection(
                &connection.preset,
                &connection.endpoint,
            ),
            base_url: connection.endpoint.clone(),
            // 指定了只影响这一次请求；没指定才沿用连接的默认模型和旧记录行为。
            model: model_id.clone(),
            max_tokens: 1,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: model::Effort::new("off"),
            tool_update: None,
        };
        let cancel = model::CancelToken::new(
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        let messages = vec![model::Message::user_text("ping")];
        let at = now_seconds();
        // 旧回退的验证要把学到的鉴权头写回连接，所以把 Keyring 的 Arc 交给来源；
        // 显式模型的验证只读连接状态，学到的头不写回。
        let keyring = std::sync::Arc::new(keyring);
        let outcome = match model::DeepSeek::with_credentials(
            config,
            std::sync::Arc::new(VerifyCredential {
                api_key,
                keyring: std::sync::Arc::clone(&keyring),
                connection: request.id.clone(),
                persist_learned_auth_header: !explicit_model,
                data_dir: self.options.data_dir.clone(),
            }),
        ) {
            Err(error) => Err(error),
            Ok(model) => model
                .stream(&messages, &model::ToolRequest::default(), &cancel)
                .await
                .map(|_| ()),
        };
        let result = ConnectionVerifyResult {
            model: model_id,
            ok: outcome.is_ok(),
            reason: outcome.as_ref().err().map(ToString::to_string),
        };
        // 指定模型只回报本次结果：不写连接 check，也不动最近选择。
        if !explicit_model {
            let check = match &outcome {
                Ok(()) => credentials::CheckResult::Ok { at },
                Err(error) => credentials::CheckResult::Fail {
                    at,
                    reason: error.code.as_str().to_owned(),
                },
            };
            keyring.record_check(&request.id, check)?;
            self.emit_settings_changed();
        }
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
        Ok(ConnectionVerifyResponse {
            connection,
            result: explicit_model.then_some(result),
        })
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
        // 「新」的基准只有上一次 fetch 成功存下的厂商列表（Magpie 的 AvailableCompare）。
        // 这个连接第一次获取、或换地址/换预设后旧表已删，都没有基准：一条都不标新。
        // query 不写盘，所以它读到的基准永远停在上一次 fetch 的那份列表上。
        let baseline: Option<std::collections::HashSet<String>> = match previous {
            Some(list) if !list.models.is_empty() => {
                Some(list.models.into_iter().map(|model| model.id).collect())
            }
            _ => None,
        };
        let mut models = catalog_models;
        let mut live = false;
        let mut live_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
        if let Ok(client) = model::http::client() {
            // endpoint 以 /anthropic 结尾时列表在 Chat base 的 /v1/models。
            let url = model::catalog::models_url(&connection.endpoint);
            let mut request = client.get(url).timeout(std::time::Duration::from_secs(5));
            // 没记过单个头时两个都发：MiniMax 国内站的 /v1/models 只认 Bearer（messages 双头
            // 都能过，所以验证不会学到单头）；照 T012.6/Magpie 的默认，列表也两个头一起发。
            let secret = key.into_secret();
            match connection.auth_header.as_deref() {
                Some("authorization") => {
                    request = request.header("authorization", format!("Bearer {secret}"));
                }
                Some("x-api-key") => {
                    request = request.header("x-api-key", secret);
                }
                _ => {
                    request = request
                        .header("x-api-key", secret.clone())
                        .header("authorization", format!("Bearer {secret}"));
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
                    tool_update: None,
                });
            }
        }
        let models = models
            .into_iter()
            .map(|model| {
                // 只有厂商这次给的、上一次 fetch 存下的列表里没有的才算「新」；
                // 目录独有项、用户行、第一次获取都不算。
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

    /// 全局暂停：先落 settings.json，再改任务效果泵的准入闸门。
    ///
    /// 顺序和 remember_selection 一样：写盘失败就不改内存，页面看到错误，不会以为已经生效。
    /// 只挡 launch 准入；relay/notice 照送，排队的效果不取消、不清队。
    pub(crate) fn set_exec_paused(&self, paused: bool) -> Result<(), ServiceError> {
        let mut settings = settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        settings.exec.paused = paused;
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        if let Some(runtime) = self.task_runtime() {
            runtime.set_exec_paused(paused);
        }
        self.emit_settings_changed();
        Ok(())
    }

    /// 席位上限：校验 1–16 后落 settings.json，再改任务效果泵的准入上限。
    ///
    /// 和 set_exec_paused 一样先写盘再改内存：写不进去就不改运行时，页面能看到错误。
    /// 改小不掐断在跑的任务，只影响之后的 launch 准入。
    pub fn set_exec_seats(&self, seats: u8) -> Result<(), ServiceError> {
        if !(settings::MIN_SEATS..=settings::MAX_SEATS).contains(&seats) {
            return Err(ServiceError::SettingsInvalid(format!(
                "席位必须在 {}–{} 之间：{seats}",
                settings::MIN_SEATS,
                settings::MAX_SEATS
            )));
        }
        let mut settings = settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        settings.exec.seats = seats;
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        if let Some(runtime) = self.task_runtime() {
            runtime.set_exec_seats(seats);
        }
        self.emit_settings_changed();
        Ok(())
    }

    /// 默认角色的 bash 开关：先落 settings.json，再改任务运行时的内存值。
    ///
    /// 和 set_exec_seats 一样先写盘再改内存：写不进去就不改运行时，页面能看到错误。
    /// 只影响之后派的任务；已派的任务冻结着自己的值（D85）。
    pub fn set_exec_bash(&self, bash: bool) -> Result<(), ServiceError> {
        let mut settings = settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        settings.exec.bash = bash;
        settings::save(&self.options.data_dir, &settings).map_err(ServiceError::SettingsInvalid)?;
        if let Some(runtime) = self.task_runtime() {
            runtime.set_exec_bash(bash);
        }
        self.emit_settings_changed();
        Ok(())
    }

    /// 写非密钥设置（币种、执行设置）。只碰 settings.json，不碰 Keyring/连接。
    ///
    /// 全部字段为 None 时只读当前值，不写文件。执行设置不是只写文件：改完文件再走
    /// set_exec_paused / set_exec_seats / set_exec_bash 更新任务运行时的状态，
    /// 页面看到的就是生效值。
    pub async fn settings_update(
        &self,
        request: SettingsUpdateRequest,
    ) -> Result<SettingsUpdateResponse, ServiceError> {
        // 先校验全部字段：任何一项不合法都不写文件，也不改运行时。
        if let Some(currency) = request.currency.as_deref()
            && currency != "usd"
            && currency != "cny"
        {
            return Err(ServiceError::SettingsInvalid(format!(
                "不支持的币种：{currency}（只有 usd/cny）"
            )));
        }
        if let Some(seats) = request.seats
            && !(settings::MIN_SEATS..=settings::MAX_SEATS).contains(&seats)
        {
            return Err(ServiceError::SettingsInvalid(format!(
                "席位必须在 {}–{} 之间：{seats}",
                settings::MIN_SEATS,
                settings::MAX_SEATS
            )));
        }

        // 读一次严格口径：文件坏了，读和写都拒绝，页面看到原因。
        let current = settings::load_for_write(&self.options.data_dir)
            .map_err(ServiceError::SettingsUnreadable)?;
        let changed = request.currency.is_some()
            || request.paused.is_some()
            || request.seats.is_some()
            || request.bash.is_some();
        if let Some(currency) = request.currency {
            let mut settings = settings::load_for_write(&self.options.data_dir)
                .map_err(ServiceError::SettingsUnreadable)?;
            settings.currency = currency;
            settings::save(&self.options.data_dir, &settings)
                .map_err(ServiceError::SettingsInvalid)?;
        }
        if let Some(paused) = request.paused {
            self.set_exec_paused(paused)?;
        }
        if let Some(seats) = request.seats {
            self.set_exec_seats(seats)?;
        }
        if let Some(bash) = request.bash {
            self.set_exec_bash(bash)?;
        }

        let settings = if changed {
            settings::load_for_write(&self.options.data_dir)
                .map_err(ServiceError::SettingsUnreadable)?
        } else {
            current
        };
        Ok(SettingsUpdateResponse {
            currency: settings.currency,
            exec: ExecSettingsView {
                paused: settings.exec.paused,
                // 手改文件写出 0/17 时回送夹回后的值，和准入用的上限一致。
                seats: settings
                    .exec
                    .seats
                    .clamp(settings::MIN_SEATS, settings::MAX_SEATS),
                bash: settings.exec.bash,
            },
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
/// 旧回退的验证把学到的鉴权头写回连接（和对话路径同一接缝）；显式模型的验证只读。
struct VerifyCredential {
    api_key: String,
    keyring: std::sync::Arc<credentials::Keyring>,
    connection: String,
    /// 适配器学到的鉴权头要不要写回连接；显式模型的验证只回报本次结果，不写。
    persist_learned_auth_header: bool,
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
        if !self.persist_learned_auth_header {
            return;
        }
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

pub(crate) fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}
