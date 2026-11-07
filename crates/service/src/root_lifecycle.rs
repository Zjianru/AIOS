//! 根会话生命周期：打开/安装/重开根会话，维护 ready 状态与上下文占用。

use std::path::Path;
use std::sync::{Arc, Mutex};

use agent::Agent;
use tokio::sync::broadcast;

use crate::SERVICE_EVENT_CHANNEL_CAPACITY;
use crate::events::context_used;
use crate::protocol::{
    ContextUsage, HostInfo, HostLocation, PROTOCOL_VERSION, RootState, RootUnavailableKind,
    ServiceEvent,
};
use crate::root::{RootError, RootSessionOptions, open_root_session, session_model_selection};
use crate::session_host::ServiceHost;

impl RootState {
    /// 把打开根会话的错误分类成页面状态（只增：error 的 Debug/说明仍保留）。
    pub fn from_root_error(error: &RootError) -> Self {
        match error {
            RootError::StateCorrupt { path, line, .. } => RootState::Unavailable {
                kind: RootUnavailableKind::Corrupt,
                path: Some(path.clone()),
                line: *line,
                message: error.to_string(),
            },
            RootError::RecordedSessionMissing { path, .. } => RootState::Unavailable {
                kind: RootUnavailableKind::Lost,
                path: Some(path.clone()),
                line: None,
                message: error.to_string(),
            },
            RootError::WriterHeld { .. } => RootState::Unavailable {
                kind: RootUnavailableKind::Busy,
                path: None,
                line: None,
                message: error.to_string(),
            },
            RootError::Agent(_) | RootError::Io(_) => RootState::Unavailable {
                kind: RootUnavailableKind::Other,
                path: None,
                line: None,
                message: error.to_string(),
            },
        }
    }
}

/// 重开根会话的两种来历：根会话不可用后的恢复，或空闲时跨连接换模型。
#[derive(Clone, Copy)]
pub(crate) enum ReopenMode {
    /// 恢复（recheck）：Keyring 可能变了；窗口和占用一起重置。
    Restore,
    /// 热切换（跨连接选模型）：Keyring 没变；只换窗口，占用保留。
    HotSwitch,
}

/// 根会话宿主：根会话可用时转发一个 ServiceHost；不可用时服务照常起，
/// ready 报状态、方法一律拒绝，recheck 修好后接管。协议方法只增不改。
pub struct RootHost {
    pub(crate) inner: tokio::sync::Mutex<Option<ServiceHost>>,
    pub(crate) state: Mutex<RootState>,
    session_id: Mutex<Option<String>>,
    pub(crate) context: Arc<Mutex<ContextUsage>>,
    pub(crate) options: RootSessionOptions,
    pub(crate) window: Mutex<u64>,
    pub(crate) events: broadcast::Sender<ServiceEvent>,
    agent_events: broadcast::Sender<agent::AgentEvent>,
    forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    agent_forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 当前生效的模型选择（连接/模型/想要的档）；session/model 的 durable 事实
    /// 在会话日志里，这里只是进程内缓存（重开从日志恢复）。
    pub(crate) selection: Mutex<Option<session::ModelSelection>>,
}

impl RootHost {
    /// 打开根会话；打不开也返回宿主（状态进 ready，方法拒绝），由页面显示说明页。
    ///
    /// 配置了 model_factory 时先解析当前 Keyring 连接；解析失败（空库/读不出）
    /// 仍返回宿主，状态报 Other，设置/首次使用方法照常可读。
    pub async fn open(mut options: RootSessionOptions, fallback_window: u64) -> Self {
        let (events, _) = broadcast::channel(SERVICE_EVENT_CHANNEL_CAPACITY);
        let (agent_events, _) = broadcast::channel(SERVICE_EVENT_CHANNEL_CAPACITY);
        let mut factory_error = None;
        let mut selection = None;
        if let Some(factory) = options.model_factory.clone() {
            // 重开会话从日志恢复选择；新会话由实现方按 settings/默认连接解析。
            let wanted = session_model_selection(&options.data_dir);
            match factory(wanted.as_ref()) {
                Ok((connection, model, credentials)) => {
                    selection = Some(session::ModelSelection {
                        connection,
                        model: model.model.clone(),
                        effort_wanted: model.effort.as_str().to_owned(),
                    });
                    options.model = model;
                    options.credentials = credentials;
                }
                Err(message) => factory_error = Some(message),
            }
        } else {
            // 无 model_factory（测试/定制宿主）：从占位配置推初始选择，同样写成 durable 事实。
            selection = Some(session::ModelSelection {
                connection: "deepseek".to_owned(),
                model: options.model.model.clone(),
                effort_wanted: options.model.effort.as_str().to_owned(),
            });
        }
        let window = if options.model_factory.is_some() {
            options.model.context_window
        } else {
            fallback_window
        };
        // 把初始选择写成 durable 事实（open/首次打开时若日志里没有）。
        let initial_selection = selection.clone();
        let host = Self {
            inner: tokio::sync::Mutex::new(None),
            state: Mutex::new(RootState::Ok),
            session_id: Mutex::new(None),
            context: Arc::new(Mutex::new(ContextUsage { used: 0, window })),
            options,
            window: Mutex::new(window),
            events,
            agent_events,
            forward: Mutex::new(None),
            agent_forward: Mutex::new(None),
            selection: Mutex::new(selection),
        };
        if let Some(message) = factory_error {
            *host.state.lock().expect("root state 锁被毒化") = RootState::Unavailable {
                kind: RootUnavailableKind::Other,
                path: None,
                line: None,
                message,
            };
        } else {
            match open_root_session(host.options.clone()).await {
                Ok(root) => {
                    ensure_initial_selection(&root.agent, initial_selection.as_ref()).await;
                    host.install(root.agent).await;
                }
                Err(error) => {
                    *host.state.lock().expect("root state 锁被毒化") =
                        RootState::from_root_error(&error);
                }
            }
        }
        host
    }

    /// 接手一个已打开的 Agent：建 ServiceHost、把它的服务事件转发到本宿主的广播，
    /// 同时更新上下文占用（页面要的 context 只从日志事实算）。
    async fn install(&self, agent: Agent) {
        let session_id = agent.id().as_str().to_owned();
        // 流式文本等 AgentEvent 不落日志，单独转发给载体（只增帧类型）。
        let mut agent_source = agent.subscribe();
        let agent_tx = self.agent_events.clone();
        let agent_forward = tokio::spawn(async move {
            loop {
                match agent_source.recv().await {
                    Ok(event) => {
                        let _ = agent_tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        if let Some(handle) = self
            .agent_forward
            .lock()
            .expect("agent forward 锁被毒化")
            .replace(agent_forward)
        {
            handle.abort();
        }
        let service_host = ServiceHost::new(agent);
        let mut source = service_host.subscribe_events();
        let tx = self.events.clone();
        let context = Arc::clone(&self.context);
        let window = *self.window.lock().expect("window 锁被毒化");
        let forward = tokio::spawn(async move {
            loop {
                match source.recv().await {
                    Ok(event) => {
                        if let Some(used) = context_used(&event) {
                            *context.lock().expect("context 锁被毒化") =
                                ContextUsage { used, window };
                        }
                        let _ = tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        if let Some(handle) = self
            .forward
            .lock()
            .expect("forward 锁被毒化")
            .replace(forward)
        {
            handle.abort();
        }
        *self.inner.lock().await = Some(service_host);
        *self.state.lock().expect("root state 锁被毒化") = RootState::Ok;
        *self.session_id.lock().expect("session id 锁被毒化") = Some(session_id);
    }

    /// 宿主信息（ready/status 用）：协议版本 + 根状态 + 上下文占用。
    pub fn info(&self) -> HostInfo {
        let context = *self.context.lock().expect("context 锁被毒化");
        HostInfo {
            protocol_version: PROTOCOL_VERSION,
            session_id: self.session_id.lock().expect("session id 锁被毒化").clone(),
            root: self.state.lock().expect("root state 锁被毒化").clone(),
            context,
            host: HostLocation {
                home: std::env::var("HOME").ok().filter(|home| !home.is_empty()),
            },
        }
    }

    pub fn protocol_version(&self) -> u64 {
        PROTOCOL_VERSION
    }

    /// 会话工作目录（本地文件路由拿它当根；只借用，不复制）。
    pub fn cwd(&self) -> &Path {
        &self.options.cwd
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }

    /// AgentEvent 的 live 订阅（流式文本/用量等不落日志的观察面）。
    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<agent::AgentEvent> {
        self.agent_events.subscribe()
    }

    /// 再检查一次：修好之后接管根会话并返回新状态；还是不行就返回当前状态。
    pub async fn recheck(&self) -> RootState {
        if self.state.lock().expect("root state 锁被毒化").clone() == RootState::Ok {
            return RootState::Ok;
        }
        let wanted = self.selection.lock().expect("selection 锁被毒化").clone();
        match self.reopen(wanted.as_ref(), ReopenMode::Restore).await {
            Ok(()) => RootState::Ok,
            Err(state) => state,
        }
    }

    /// 重开根会话：按想要的选择解析连接，放掉旧 Agent（释放会话写锁），打开同一条根会话，
    /// 装回宿主。失败时把不可用状态写回；调用方决定报错（跨连接切换）还是只报状态（recheck）。
    ///
    /// `wanted` = None 表示没有会话选择（新会话），实现方按 settings/默认连接解析。
    /// 恢复会重置窗口与占用并通知 Keyring；热切换只换窗口，占用是同一会话日志的事实。
    pub(crate) async fn reopen(
        &self,
        wanted: Option<&session::ModelSelection>,
        mode: ReopenMode,
    ) -> Result<(), RootState> {
        let mut options = self.options.clone();
        if let Some(factory) = options.model_factory.clone() {
            match factory(wanted) {
                Ok((connection, model, credentials)) => {
                    *self.selection.lock().expect("selection 锁被毒化") =
                        Some(session::ModelSelection {
                            connection,
                            model: model.model.clone(),
                            effort_wanted: model.effort.as_str().to_owned(),
                        });
                    options.model = model;
                    options.credentials = credentials;
                }
                Err(message) => {
                    let state = RootState::Unavailable {
                        kind: RootUnavailableKind::Other,
                        path: None,
                        line: None,
                        message,
                    };
                    *self.state.lock().expect("root state 锁被毒化") = state.clone();
                    return Err(state);
                }
            }
        }
        // 新连接可能换模型/窗口；恢复时窗口和占用一起重置，热切换只换窗口。
        let window = options.model.context_window;
        if matches!(mode, ReopenMode::Restore) {
            self.notify_keyring_changed();
            *self.window.lock().expect("window 锁被毒化") = window;
            *self.context.lock().expect("context 锁被毒化") = ContextUsage { used: 0, window };
        }
        // 先放掉旧 Agent（释放会话写锁）再重开同一会话；recheck 时通常已经是 None。
        let old = self.inner.lock().await.take();
        drop(old);
        match open_root_session(options).await {
            Ok(root) => {
                if matches!(mode, ReopenMode::HotSwitch) {
                    // 占用是同一会话日志里的事实，换连接的窗口只影响之后算比例。
                    *self.window.lock().expect("window 锁被毒化") = window;
                    self.context.lock().expect("context 锁被毒化").window = window;
                }
                let current = self.selection.lock().expect("selection 锁被毒化").clone();
                ensure_initial_selection(&root.agent, current.as_ref()).await;
                self.install(root.agent).await;
                Ok(())
            }
            Err(error) => {
                let state = RootState::from_root_error(&error);
                *self.state.lock().expect("root state 锁被毒化") = state.clone();
                Err(state)
            }
        }
    }
}

/// 会话日志里没有 session/model 事件时，把当前选择写成第一条 durable 事实；
/// 重开时第一条 session/model 就是真正初始选择（stats 逐轮定价用）。
async fn ensure_initial_selection(agent: &Agent, selection: Option<&session::ModelSelection>) {
    let Some(selection) = selection else {
        return;
    };
    if session::last_model_selection(&agent.session_events_from(0)).is_some() {
        return;
    }
    let _ = agent.select(selection).await;
}

impl Drop for RootHost {
    fn drop(&mut self) {
        if let Some(handle) = self.forward.lock().expect("forward 锁被毒化").take() {
            handle.abort();
        }
    }
}
