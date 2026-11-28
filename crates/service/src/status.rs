//! 状态组装：把宿主、选择、Keyring、统计与汇率合成 StatusResponse。

use crate::error::ServiceError;
use crate::protocol::{StatusRequest, StatusResponse};
use crate::root_lifecycle::RootHost;
use crate::stats;

impl RootHost {
    /// 状态（对象/在跑/待答问题/工作目录）+ 会话统计（轮/步/token/缓存/上下文/花费）。
    ///
    /// 统计和花费从日志现算（不落日志）：每轮模型取 assistant/message 的 message.source.model，
    /// 价格按当前连接 + 目录（用户价 > 目录标价 > 无价格；无价格的轮标 unpriced）。
    pub(crate) async fn host_status(
        &self,
        _request: StatusRequest,
    ) -> Result<StatusResponse, ServiceError> {
        // 日志快照和工作目录只在这把锁里取：Agent 一旦被克隆带出去，会话写锁就会跟着
        // 跨过下面的 await 时不能还拿着会话写锁，否则重开根会话会被误报 writer-held。
        let (object, running, pending_questions, events, cwd) = {
            let guard = self.inner.lock().await;
            let Some(host) = guard.as_ref() else {
                return Err(ServiceError::RootUnavailable(
                    self.state.lock().expect("root state 锁被毒化").clone(),
                ));
            };
            let agent = host.agent();
            (
                host.object(),
                agent.is_running(),
                agent.pending_questions().len(),
                agent.session_events_from(0),
                agent.cwd().to_string_lossy().into_owned(),
            )
        };
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
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == current.connection)
            .or_else(|| view.connections.first())
            .cloned();
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let (context_used, context_window) = {
            let context = self.context.lock().expect("context 锁被毒化");
            (context.used, context.window)
        };
        let stats = match connection {
            Some(_) => stats::session_stats(
                &events,
                &view.connections,
                &catalog,
                &current,
                context_used,
                context_window,
            ),
            None => stats::SessionStats {
                context_used,
                context_window,
                connection: current.connection.clone(),
                model: current.model.clone(),
                effort_wanted: current.effort_wanted.clone(),
                ..stats::SessionStats::default()
            },
        };
        let mut stats = stats;
        stats.effort_actual = connection
            .as_ref()
            .map(|connection| stats::actual_effort_of(connection, &current))
            .unwrap_or_else(|| current.effort_wanted.clone());
        Ok(StatusResponse {
            object,
            running,
            pending_questions,
            stats: Some(stats),
            cwd: Some(cwd),
        })
    }
}
