//! 选择模型：写会话日志与 settings.json，空闲跨连接时重开根会话。

use crate::error::ServiceError;
use crate::protocol::{SessionModelSelectRequest, SessionModelSelectResponse};
use crate::root_lifecycle::{ReopenMode, RootHost};
use crate::stats;

impl RootHost {
    /// 选模型（连接/模型/想要的档；下一轮生效，D26）。
    ///
    /// 选择是 durable 事实：写 session/model 事件（重开会话从日志恢复）并更新 settings.json
    /// 的上次选择（新会话继承）。同连接换模型/档只改下一轮；跨连接时若当前空闲则重建根
    /// agent（新连接从下一轮开始用），已用上下文保留、不重复通知 Keyring；正在跑一轮时只
    /// 记录选择，根会话仍用旧连接（known：运行中跨连接热切换不打断当前轮）。这时的恢复
    /// 路径是重启，或空闲时先选另一个连接再选回来；recheck 在根状态 Ok 时不重建。
    pub async fn session_model_select(
        &self,
        request: SessionModelSelectRequest,
    ) -> Result<SessionModelSelectResponse, ServiceError> {
        let current = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .unwrap_or_else(|| session::ModelSelection {
                connection: "deepseek".to_owned(),
                model: self.options.model.model.clone(),
                effort_wanted: self.options.model.effort.as_str().to_owned(),
            });
        let target = session::ModelSelection {
            connection: request
                .connection
                .unwrap_or_else(|| current.connection.clone()),
            model: request.model.unwrap_or_else(|| current.model.clone()),
            effort_wanted: request
                .effort_wanted
                .unwrap_or_else(|| current.effort_wanted.clone()),
        };
        // 先写 durable 设置（新会话默认用上一次选的）：写失败就不动内存和会话日志，
        // 页面收到 settings/unavailable，不会以为已经换过去了。
        self.remember_selection(&target)?;
        let cross_connection = target.connection != current.connection;
        let idle = {
            let guard = self.inner.lock().await;
            let Some(host) = guard.as_ref() else {
                return Err(ServiceError::RootUnavailable(
                    self.state.lock().expect("root state 锁被毒化").clone(),
                ));
            };
            host.agent().select(&target).await?;
            !host.agent().is_running()
        };
        if cross_connection && idle {
            self.reopen(Some(&target), ReopenMode::HotSwitch)
                .await
                .map_err(ServiceError::RootUnavailable)?;
        }
        // 响应报实际生效的选择（跨连接重建后 factory 解析的 connection 可能不同）。
        let actual = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .unwrap_or(target);
        // 窗口跟实际生效的模型：同一连接换模型不重建根 agent，重开时 factory 才会换窗口。
        if let Some(window) = self.model_context_window(&actual) {
            self.context.lock().expect("context 锁被毒化").window = window;
            let guard = self.inner.lock().await;
            if let Some(host) = guard.as_ref() {
                host.agent().set_context_window(window);
            }
        }
        let effort_actual = self
            .keyring()
            .ok()
            .and_then(|keyring| {
                let view = keyring.view();
                view.connections
                    .iter()
                    .find(|connection| connection.id == actual.connection)
                    .map(|connection| stats::actual_effort_of(connection, &actual))
            })
            .unwrap_or_else(|| actual.effort_wanted.clone());
        Ok(SessionModelSelectResponse {
            connection: actual.connection,
            model: actual.model,
            effort_wanted: actual.effort_wanted,
            effort_actual,
        })
    }

    /// 这个 (连接, 模型) 在 Keyring 当前视图里的上下文窗口；显式模型行优先，
    /// 其次厂商存下的列表/内置目录。解析不出来时保留现有窗口。
    fn model_context_window(&self, selection: &session::ModelSelection) -> Option<u64> {
        let keyring = self.keyring().ok()?;
        let view = keyring.view();
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == selection.connection)?;
        crate::models::selectable_models(&self.options.data_dir, connection)
            .into_iter()
            .find(|info| info.id == selection.model)
            .and_then(|info| info.context_window)
    }
}
