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
    /// 根会话槽；TaskRuntime 的 relay 也拿这一份，好按「Kind 是不是当前根会话」决定 Later。
    pub(crate) inner: Arc<tokio::sync::Mutex<Option<ServiceHost>>>,
    pub(crate) state: Mutex<RootState>,
    session_id: Mutex<Option<String>>,
    pub(crate) context: Arc<Mutex<ContextUsage>>,
    pub(crate) options: RootSessionOptions,
    pub(crate) events: broadcast::Sender<ServiceEvent>,
    agent_events: broadcast::Sender<agent::AgentEvent>,
    forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    agent_forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// T024.3 的任务执行者；None = 任务库打不开（根会话照常可用，但没有任务工具）。
    task_runtime: Option<Arc<crate::executor::TaskRuntime>>,
    task_pump: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 当前生效的模型选择（连接/模型/想要的档）；session/model 的 durable 事实
    /// 在会话日志里，这里只是进程内缓存（重开从日志恢复）。
    pub(crate) selection: Arc<Mutex<Option<session::ModelSelection>>>,
    /// 每张确认卡一把锁：同一 call_id 的并发/连点串行，第二个请求读得到第一条决定。
    pub(crate) card_locks: Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 根会话装配时用的连接 id；只在安装/重建时更新，运行中不跟着 selection 变。
    pub(crate) root_connection: Mutex<Option<String>>,
}

impl RootHost {
    /// 打开根会话；打不开也返回宿主（状态进 ready，方法拒绝），由页面显示说明页。
    ///
    /// 配置了 model_factory 时先解析当前 Keyring 连接；解析失败（空库/读不出）
    /// 仍返回宿主，状态报 Other，设置/首次使用方法照常可读。
    pub async fn open(mut options: RootSessionOptions, fallback_window: u64) -> Self {
        if options.web_listen.is_none() {
            options.web_listen = Some(Arc::new(std::sync::OnceLock::new()));
        }
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
        // 窗口跟当前选中的模型走：工厂配置里的 context_window 是 CLI 的值，等于产品
        // 默认值就表示「没显式指定」，按 Keyring 的模型信息解析真实窗口；解析不出来
        // 才退回装配配置。非默认值是显式覆盖（开发/DoD 的 --context-window），保留它
        // （e2e 靠它压小窗口）。无工厂的定制宿主沿用 fallback_window。
        let window = if options.model_factory.is_some() {
            explicit_context_window(&options.model)
                .or_else(|| {
                    selection.as_ref().and_then(|selection| {
                        crate::session_select::model_context_window(
                            &options.data_dir,
                            options.identity.as_deref(),
                            selection,
                        )
                    })
                })
                .unwrap_or(options.model.context_window)
        } else {
            fallback_window
        };
        // 把初始选择写成 durable 事实（open/首次打开时若日志里没有）。
        let initial_selection = selection.clone();
        let inner = Arc::new(tokio::sync::Mutex::new(None));
        let selection = Arc::new(Mutex::new(selection));
        // 任务执行者和根会话共用选择与根会话槽；任务库打不开不影响 Kind 的对话。
        let task_runtime =
            match crate::executor::TaskRuntime::new(&options, Arc::clone(&selection), Arc::clone(&inner))
            {
                Ok(runtime) => Some(Arc::new(runtime)),
                Err(error) => {
                    eprintln!("任务库打不开，任务工具不装：{error}");
                    None
                }
            };
        options.task_runtime = task_runtime.clone();
        let host = Self {
            inner,
            state: Mutex::new(RootState::Ok),
            session_id: Mutex::new(None),
            context: Arc::new(Mutex::new(ContextUsage { used: 0, window })),
            options,
            events,
            agent_events,
            forward: Mutex::new(None),
            agent_forward: Mutex::new(None),
            task_runtime: task_runtime.clone(),
            task_pump: Mutex::new(None),
            selection,
            card_locks: Mutex::new(std::collections::HashMap::new()),
            root_connection: Mutex::new(None),
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
                    // 压缩阈值从第一轮起就按解析出的窗口算：先设到 agent 上，再装宿主。
                    root.agent.set_context_window(window);
                    match ensure_initial_selection(&root.agent, initial_selection.as_ref()).await {
                        Ok(()) => host.install(root.agent).await,
                        Err(state) => {
                            *host.state.lock().expect("root state 锁被毒化") = state;
                        }
                    }
                }
                Err(error) => {
                    *host.state.lock().expect("root state 锁被毒化") =
                        RootState::from_root_error(&error);
                }
            }
        }
        if let Some(runtime) = task_runtime {
            let pump = runtime.start_pump();
            *host.task_pump.lock().expect("task pump 锁被毒化") = Some(pump);
        }
        host
    }

    /// T024.3 的任务执行者（测试用：暂停泵、模拟 relay 前停机）。
    #[doc(hidden)]
    pub fn task_runtime(&self) -> Option<&Arc<crate::executor::TaskRuntime>> {
        self.task_runtime.as_ref()
    }

    /// 服务绑定监听后记下实际地址；web_fetch 读它拒绝访问 AIOS 自己。
    pub fn record_listen_address(&self, address: std::net::SocketAddr) {
        if let Some(cell) = self.options.web_listen.as_ref() {
            cell.set(address).unwrap_or(());
        }
    }

    /// 接手一个已打开的 Agent：建 ServiceHost、把它的服务事件转发到本宿主的广播，
    /// 同时更新上下文占用（页面要的 context 只从日志事实算）。
    async fn install(&self, agent: Agent) {
        let mut slot = self.inner.lock().await;
        self.install_into(&mut slot, agent);
    }

    /// 在调用方已经拿住的根会话槽里装宿主。重开时槽要一直拿着，等装完才让别的调用看见
    /// 新宿主（不然 status 会撞见 take 与 install 之间的空窗）。
    fn install_into(&self, slot: &mut Option<ServiceHost>, agent: Agent) {
        let session_id = agent.id().as_str().to_owned();
        // 这一份根会话绑定的连接：之后运行中换选择不改它，直到下次安装/重建。
        let root_connection = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .as_ref()
            .map(|selection| selection.connection.clone());
        *self
            .root_connection
            .lock()
            .expect("root connection 锁被毒化") = root_connection;
        // 流式文本等 AgentEvent 不落日志，单独转发给载体（只增帧类型）。
        let mut agent_source = agent.subscribe();
        let agent_tx = self.agent_events.clone();
        let agent_forward = tokio::spawn(async move {
            loop {
                match agent_source.recv().await {
                    Ok(event) => {
                        #[expect(
                            clippy::let_underscore_must_use,
                            reason = "下游订阅端已关时继续消费到源关闭；事件已从源广播"
                        )]
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
        let forward = tokio::spawn(async move {
            loop {
                match source.recv().await {
                    Ok(event) => {
                        if let Some(used) = context_used(&event) {
                            // 只更新占用；窗口是换模型时更新的另一个事实，旧事件不能盖回去。
                            context.lock().expect("context 锁被毒化").used = used;
                        }
                        #[expect(
                            clippy::let_underscore_must_use,
                            reason = "下游订阅端已关时继续消费到源关闭；事件已从源广播"
                        )]
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
        *slot = Some(service_host);
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
        // 和 open 同口径：先看显式覆盖，再按 Keyring 里当前选择解析模型窗口。
        let window = if options.model_factory.is_some() {
            explicit_context_window(&options.model)
                .or_else(|| {
                    self.selection
                        .lock()
                        .expect("selection 锁被毒化")
                        .as_ref()
                        .and_then(|selection| {
                            crate::session_select::model_context_window(
                                &options.data_dir,
                                options.identity.as_deref(),
                                selection,
                            )
                        })
                })
                .unwrap_or(options.model.context_window)
        } else {
            options.model.context_window
        };
        if matches!(mode, ReopenMode::Restore) {
            self.notify_keyring_changed();
            *self.context.lock().expect("context 锁被毒化") = ContextUsage { used: 0, window };
        }
        // 先放掉旧 Agent（释放会话写锁）再重开同一会话；交换期间一直拿着根会话槽：
        // 别的调用（尤其 status）等装完再读，不会撞见 take 与 install 之间的 inner=None。
        let mut slot = self.inner.lock().await;
        let old = slot.take();
        drop(old);
        match open_root_session(options).await {
            Ok(root) => {
                // 重开后的第一轮也要按新窗口算压缩阈值，不能等 agent 撞一次溢出。
                root.agent.set_context_window(window);
                let current = self.selection.lock().expect("selection 锁被毒化").clone();
                if let Err(state) = ensure_initial_selection(&root.agent, current.as_ref()).await {
                    *self.state.lock().expect("root state 锁被毒化") = state.clone();
                    return Err(state);
                }
                if matches!(mode, ReopenMode::HotSwitch) {
                    // 占用是同一会话日志里的事实，换连接的窗口只影响之后算比例。
                    self.context.lock().expect("context 锁被毒化").window = window;
                }
                self.install_into(&mut slot, root.agent);
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

/// 工厂返回的窗口是 CLI 配置：等于产品默认值时视为「没有显式指定」，调用方按
/// Keyring 里的模型信息解析真实窗口；非默认值是开发/DoD 的显式覆盖，优先保留。
fn explicit_context_window(config: &model::DeepSeekConfig) -> Option<u64> {
    (config.context_window != model::DEFAULT_CONTEXT_WINDOW).then_some(config.context_window)
}

/// 初始模型选择写不进会话日志时的可见状态：recheck/ready 会带着原因拒绝方法。
pub(crate) fn initial_selection_state(error: &agent::AgentError) -> RootState {
    RootState::Unavailable {
        kind: RootUnavailableKind::Other,
        path: None,
        line: None,
        message: format!("初始模型选择没写成：{error}"),
    }
}

/// 会话日志里没有 session/model 事件时，把当前选择写成第一条 durable 事实；
/// 重开时第一条 session/model 就是真正初始选择（stats 逐轮定价用）。
async fn ensure_initial_selection(
    agent: &Agent,
    selection: Option<&session::ModelSelection>,
) -> Result<(), RootState> {
    let Some(selection) = selection else {
        return Ok(());
    };
    if session::last_model_selection(&agent.session_events_from(0)).is_some() {
        return Ok(());
    }
    agent
        .select(selection)
        .await
        .map_err(|error| initial_selection_state(&error))
}

impl Drop for RootHost {
    fn drop(&mut self) {
        if let Some(handle) = self.forward.lock().expect("forward 锁被毒化").take() {
            handle.abort();
        }
        if let Some(handle) = self.agent_forward.lock().expect("agent forward 锁被毒化").take() {
            handle.abort();
        }
        if let Some(handle) = self.task_pump.lock().expect("task pump 锁被毒化").take() {
            handle.abort();
        }
        // 任务工具持有 TaskRuntime，TaskRuntime 又拿着根会话槽：不在这里清掉槽会成环。
        if let Ok(mut slot) = self.inner.try_lock() {
            let _ = slot.take();
        }
    }
}
