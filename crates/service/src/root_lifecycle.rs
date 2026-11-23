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
use crate::root::{RootError, RootSessionOptions, open_root_session_with, session_model_selection};
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
    /// 项目库；打不开时项目方法返回不可用，根会话照常。
    projects: Option<Arc<tasks::ProjectStore>>,
    task_pump: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// 当前生效的模型选择（连接/模型/想要的档）；session/model 的 durable 事实
    /// 在会话日志里，这里只是进程内缓存（重开从日志恢复）。
    pub(crate) selection: Arc<Mutex<Option<session::ModelSelection>>>,
    /// 每张确认卡一把锁：同一 call_id 的并发/连点串行，第二个请求读得到第一条决定。
    pub(crate) card_locks: Mutex<std::collections::HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// 根会话装配时用的连接 id；只在安装/重建时更新，运行中不跟着 selection 变。
    pub(crate) root_connection: Mutex<Option<String>>,
    /// 记忆库（根会话与做梦共用一份；打不开就没有记忆能力）。
    memory: Option<Arc<crate::memory::MemoryRuntime>>,
    /// 做梦运行时；根会话可用时由它按时机检查，不常驻。
    dream: Option<Arc<crate::memory::DreamRuntime>>,
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
        let projects = tasks::ProjectStore::open(options.data_dir.join(tasks::PROJECTS_DB_FILE))
            .ok()
            .map(Arc::new);
        options.projects = projects.clone();
        let task_runtime = match crate::executor::TaskRuntime::new(
            &options,
            Arc::clone(&selection),
            Arc::downgrade(&inner),
            Some(events.clone()),
        ) {
            Ok(runtime) => Some(Arc::new(runtime)),
            Err(error) => {
                eprintln!("任务库打不开，任务工具不装：{error}");
                None
            }
        };
        options.task_runtime = task_runtime.clone();
        let memory = crate::memory::MemoryRuntime::open(&options.data_dir)
            .ok()
            .map(Arc::new);
        let dream = memory.as_ref().map(|memory| {
            Arc::new(crate::memory::DreamRuntime::new(
                &options.data_dir,
                &options.cwd,
                Arc::clone(memory),
                Arc::clone(&inner),
                Arc::clone(&selection),
                options.model_factory.clone(),
                options.model.clone(),
                options.credentials.clone(),
                task_runtime.clone(),
            ))
        });
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
            projects,
            task_pump: Mutex::new(None),
            selection,
            card_locks: Mutex::new(std::collections::HashMap::new()),
            root_connection: Mutex::new(None),
            memory,
            dream,
        };
        // 做梦开始/结束时推 memory/changed，记忆页收到就重读（不靠轮询）。
        if let (Some(dream), Some(memory)) = (&host.dream, &host.memory) {
            let events = host.events.clone();
            let memory = Arc::clone(memory);
            dream.set_changed_hook(Arc::new(move || {
                let version = memory.store().version().unwrap_or(0);
                let event = ServiceEvent {
                    seq: 0,
                    time: 0,
                    event_type: "memory/changed".to_owned(),
                    data: serde_json::json!({ "version": version }),
                    cause: None,
                };
                let _delivered = events.send(event).unwrap_or(0);
            }));
        }
        if let Some(message) = factory_error {
            *host.state.lock().expect("root state 锁被毒化") = RootState::Unavailable {
                kind: RootUnavailableKind::Other,
                path: None,
                line: None,
                message,
            };
        } else {
            match open_root_session_with(
                host.options.clone(),
                host.memory.clone(),
                host.dream.clone(),
            )
            .await
            {
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
            // 另一个活着的服务占着根会话（writer-held）时这不算一次启动：在跑的任务还在它手里，
            // 不能扫成打断。其余情况（含模型/凭证没接上）照扫，restart 效果留在库里等根会话好了再送。
            let busy = matches!(
                *host.state.lock().expect("root state 锁被毒化"),
                RootState::Unavailable {
                    kind: RootUnavailableKind::Busy,
                    ..
                }
            );
            if !busy && let Err(error) = runtime.scan_at_boot(&new_boot_id()) {
                eprintln!("启动扫描失败：{error}");
            }
            runtime.calendar_recover();
            let pump = runtime.start_pump();
            *host.task_pump.lock().expect("task pump 锁被毒化") = Some(pump);
        }
        // 服务启动是一个做梦检查时机；条件到才跑，不影响打开。
        if let Some(dream) = &host.dream {
            dream.trigger(crate::memory::DreamTrigger::Startup);
        }
        host
    }

    /// T024.3 的任务执行者（测试用：暂停泵、模拟 relay 前停机）。
    #[doc(hidden)]
    pub fn task_runtime(&self) -> Option<&Arc<crate::executor::TaskRuntime>> {
        self.task_runtime.as_ref()
    }

    /// 项目库；打不开时 None。
    pub(crate) fn projects(&self) -> Option<&Arc<tasks::ProjectStore>> {
        self.projects.as_ref()
    }

    /// 测试同步：根 Kind 当前不在跑（日历复核要等它空下来才不判 Kind 忙）。
    #[doc(hidden)]
    pub async fn root_agent_idle(&self) -> bool {
        let guard = self.inner.lock().await;
        guard.as_ref().is_none_or(|host| !host.agent().is_running())
    }

    /// 当前 Kind 根会话的轮号（确认卡 act 记来源轮用）。
    pub(crate) async fn current_root_turn(&self) -> Option<u64> {
        let guard = self.inner.lock().await;
        let host = guard.as_ref()?;
        host.agent()
            .session_events_from(0)
            .iter()
            .rev()
            .find_map(|event| match &event.kind {
                session::EventKind::TurnStart { turn } => Some(*turn),
                _ => None,
            })
    }

    /// 根 WS 转发任务事实变化（task-changed）用；没有任务运行时为空。
    pub fn subscribe_task_changed(&self) -> Option<tokio::sync::broadcast::Receiver<String>> {
        self.task_runtime
            .as_ref()
            .map(|runtime| runtime.subscribe_task_changed())
    }

    /// 项目库变了：走 task-changed 推送，让页面重读项目 store（不推进 lastSeq）。
    pub(crate) fn notify_projects_changed(&self) {
        if let Some(runtime) = self.task_runtime.as_ref() {
            runtime.notify_projects_changed();
        }
    }

    /// 测试钩子：循环约定的 task_continue 准入原因（有就返回 Debug 文本）。
    #[doc(hidden)]
    pub fn calendar_cycle_cause(&self, task_id: &str) -> Option<String> {
        self.task_runtime.as_ref().and_then(|runtime| {
            runtime
                .calendar_cycle_continue_cause_for(task_id)
                .ok()
                .flatten()
                .map(|cause| format!("{cause:?}"))
        })
    }

    /// 测试钩子：模拟根会话里一条 task-report 送到。
    #[doc(hidden)]
    pub fn calendar_task_report(&self, task_id: &str) {
        if let Some(runtime) = &self.task_runtime {
            runtime.calendar_on_task_report(task_id);
        }
    }

    /// 测试夹具：URL 盯变化走给定代理。
    #[doc(hidden)]
    pub fn set_calendar_watch_proxy(&self, proxy: String) {
        if let Some(runtime) = &self.task_runtime {
            runtime.set_watch_proxy(proxy);
        }
    }

    /// 测试钩子：启动补结束轮那条路（不重启宿主也能跑）。
    #[doc(hidden)]
    pub fn calendar_close_finished_turns(&self) {
        if let Some(runtime) = &self.task_runtime
            && let Some(conversation) = self.info().session_id
        {
            runtime.calendar_close_finished_turns(&conversation);
        }
    }

    /// 测试钩子：叫醒日历泵（改夹具的 next_trigger 后不用等自然醒）。
    #[doc(hidden)]
    pub fn nudge_calendar(&self) {
        if let Some(runtime) = &self.task_runtime {
            runtime.calendar_nudge();
        }
    }

    /// 测试钩子：下一次日历唤醒送达前失败一次（退避重试用例）。
    #[doc(hidden)]
    pub fn crash_before_next_wake(&self) {
        if let Some(runtime) = &self.task_runtime {
            runtime.crash_before_next_wake();
        }
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
        let conversation_id = agent.id().as_str().to_owned();
        let memory = crate::memory::MemoryRuntime::open(&self.options.data_dir)
            .ok()
            .map(std::sync::Arc::new);
        if let Some(memory) = &memory {
            memory.backfill(&self.options.data_dir, &conversation_id);
        }
        let service_host = ServiceHost::new_with_memory(agent, memory);
        let mut source = service_host.subscribe_events();
        let tx = self.events.clone();
        let context = Arc::clone(&self.context);
        let dream = self.dream.clone();
        let calendar_runtime = self.task_runtime.clone();
        let forward_conversation = conversation_id.clone();
        let forward = tokio::spawn(async move {
            loop {
                match source.recv().await {
                    Ok(event) => {
                        if let Some(used) = context_used(&event) {
                            // 只更新占用；窗口是换模型时更新的另一个事实，旧事件不能盖回去。
                            context.lock().expect("context 锁被毒化").used = used;
                        }
                        // 做梦的两个时机：Kind 回合结束、任务报回中继。检查函数自己判条件。
                        if let Some(dream) = &dream {
                            if event.event_type == "turn/end" {
                                dream.trigger(crate::memory::DreamTrigger::TurnEnded);
                            } else if event.event_type == "user/message"
                                && event_is_task_relay(&event)
                            {
                                dream.trigger(crate::memory::DreamTrigger::TaskRelayed);
                            }
                        }
                        if event.event_type == "turn/end"
                            && let Some(runtime) = &calendar_runtime
                            && let Some(turn) =
                                event.data.get("turn").and_then(serde_json::Value::as_u64)
                        {
                            runtime.calendar_on_turn_end(
                                &forward_conversation,
                                turn,
                                i64::try_from(event.time).unwrap_or(i64::MAX),
                            );
                        }
                        if event.event_type == "user/message"
                            && let Some(runtime) = &calendar_runtime
                            && let Some(rpc_id) = event_user_rpc(&event)
                            && let Some(rest) = rpc_id.strip_prefix("task-report:")
                            && let Some(task_id) = rest.split(':').next()
                            && !task_id.is_empty()
                        {
                            runtime.calendar_on_task_report(task_id);
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
        if let Some(runtime) = &self.task_runtime {
            runtime.calendar_close_finished_turns(&conversation_id);
        }
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

    /// 页面打开/进入记忆页时顺手检查一次做梦（不常驻、不轮询）。
    pub fn dream_check(&self) {
        if let Some(dream) = &self.dream {
            dream.trigger(crate::memory::DreamTrigger::PageOpened);
        }
    }

    /// 之前触发的做梦检查都收尾了（测试同步用；生产不依赖它）。
    pub fn dream_checks_settled(&self) -> bool {
        self.dream
            .as_ref()
            .is_none_or(|dream| dream.checks_settled())
    }

    /// 马上整理一次（工具/页面都走这里）；正在跑返回「正在整理」。
    pub fn dream_now(&self) -> Result<String, String> {
        self.dream
            .as_ref()
            .ok_or_else(|| "做梦没装配".to_owned())?
            .request_now()
    }

    /// 撤销一处/整次做梦。
    pub fn dream_undo(&self, only: Option<&str>) -> Result<crate::memory::DreamReport, String> {
        self.dream
            .as_ref()
            .ok_or_else(|| "做梦没装配".to_owned())?
            .undo(only)
    }

    /// 记忆库（记忆页/做梦用）；库没打开就是 None。
    pub(crate) fn memory_runtime(&self) -> Option<&Arc<crate::memory::MemoryRuntime>> {
        self.memory.as_ref()
    }

    /// 做梦运行时（记忆页/卡片动作用）；没装配就是 None。
    pub(crate) fn dream_runtime(&self) -> Option<&Arc<crate::memory::DreamRuntime>> {
        self.dream.as_ref()
    }

    /// 记忆版本变了：seq=0 的合成事件，页面收到重读（不推进 lastSeq）。
    pub(crate) fn emit_memory_changed(&self, version: u64) {
        let event = ServiceEvent {
            seq: 0,
            time: 0,
            event_type: "memory/changed".to_owned(),
            data: serde_json::json!({ "version": version }),
            cause: None,
        };
        // 没有订阅端是正常情况（页面没开）；不把广播失败当错误。
        let _delivered = self.events.send(event).unwrap_or(0);
    }

    /// 读做梦设置。
    pub fn dream_settings(&self) -> Result<memory::types::DreamSettings, String> {
        self.memory
            .as_ref()
            .ok_or_else(|| "记忆库打不开".to_owned())?
            .store()
            .dream_settings()
            .map_err(|error| error.to_string())
    }

    /// 改做梦设置（开关/间隔/许可）。
    pub fn update_dream_settings(
        &self,
        patch: &memory::types::DreamSettingsPatch,
    ) -> Result<memory::types::DreamSettings, String> {
        self.memory
            .as_ref()
            .ok_or_else(|| "记忆库打不开".to_owned())?
            .store()
            .update_dream_settings(patch)
            .map_err(|error| error.to_string())
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
        match open_root_session_with(options, self.memory.clone(), self.dream.clone()).await {
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

/// 用户消息的 source.rpcId（程序消息/中继卡都从这里认前缀）。
fn event_user_rpc(event: &ServiceEvent) -> Option<&str> {
    event
        .data
        .get("message")
        .and_then(|message| message.get("source"))
        .and_then(|source| source.get("rpcId"))
        .and_then(|value| value.as_str())
}

fn event_is_task_relay(event: &ServiceEvent) -> bool {
    let Some(rpc_id) = event
        .data
        .get("message")
        .and_then(|message| message.get("source"))
        .and_then(|source| source.get("rpcId"))
        .and_then(|value| value.as_str())
    else {
        return false;
    };
    crate::program_messages::is_task_relay_rpc(rpc_id)
}

/// 本次启动的 id：restart 效果的键和 Kind 汇报的 request id 用它。
///
/// 时间 + 进程内序号；不需要全局唯一，只要两次启动不同就会各排一条 restart。
fn new_boot_id() -> tasks::BootId {
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();
    let serial = NEXT.fetch_add(1, Ordering::Relaxed);
    tasks::BootId::new(format!("boot-{millis}-{serial}"))
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
        if let Some(handle) = self
            .agent_forward
            .lock()
            .expect("agent forward 锁被毒化")
            .take()
        {
            handle.abort();
        }
        if let Some(handle) = self.task_pump.lock().expect("task pump 锁被毒化").take() {
            handle.abort();
        }
    }
}

/// 日历上的内置虚拟行「做梦」——不建日历库行，状态读记忆设置，运行读做梦记录。
impl RootHost {
    pub(crate) fn calendar_builtin_view(&self) -> Option<crate::protocol::CalendarAppointmentView> {
        let memory = self.memory.as_ref()?;
        let settings = memory.store().dream_settings().ok()?;
        let timezone = crate::delivery::local_timezone().name().to_owned();
        let runs = memory.store().dream_runs(5).unwrap_or_default();
        let enabled = settings.enabled;
        let interval_hours = settings.interval_hours;
        let last_at = settings.last_attempt_at.max(settings.last_success_at);
        let next_at = if enabled && last_at > 0 {
            Some(
                last_at
                    + i64::try_from(interval_hours)
                        .unwrap_or(24)
                        .saturating_mul(3_600_000),
            )
        } else {
            None
        };
        let run_views = runs
            .into_iter()
            .enumerate()
            .map(|(index, run)| crate::protocol::CalendarRunView {
                id: i64::try_from(index).unwrap_or(0) + 1,
                nominal_at: run.started_at,
                manual: true,
                trigger: "manual".to_owned(),
                result: if run.outcome == "success" {
                    "sent"
                } else {
                    "failed"
                }
                .to_owned(),
                reason: if run.error.is_empty() {
                    run.outcome
                } else {
                    run.error
                },
                merged_count: 0,
                created_at: run.started_at,
                updated_at: run.ended_at,
                sent_turn: None,
            })
            .collect();
        Some(crate::protocol::CalendarAppointmentView {
            id: "builtin:dream".to_owned(),
            title: "做梦".to_owned(),
            condition: "整理记忆（内置）".to_owned(),
            rule: format!("FREQ=HOURLY;INTERVAL={interval_hours}"),
            trigger: crate::calendar_trigger::describe(
                &format!("FREQ=HOURLY;INTERVAL={interval_hours}"),
                &timezone,
                0,
            ),
            timezone,
            status: if enabled { "active" } else { "paused" }.to_owned(),
            version: 1,
            binding: "kind".to_owned(),
            bound_task_id: None,
            next_trigger_at: next_at,
            duration_minutes: None,
            pause_reason: if enabled {
                None
            } else {
                Some("做梦已关".to_owned())
            },
            watch_kind: None,
            watch_target: None,
            cycle_task_id: None,
            cycle_max_rounds: None,
            source_turn: None,
            created_at: 0,
            updated_at: last_at,
            runs: run_views,
        })
    }

    pub(crate) fn calendar_builtin_action(
        &self,
        action: &crate::protocol::CalendarActionKind,
    ) -> Result<crate::protocol::CalendarActionResponse, String> {
        use crate::protocol::CalendarActionKind;
        match action {
            CalendarActionKind::Pause | CalendarActionKind::Resume => {
                let memory = self
                    .memory
                    .as_ref()
                    .ok_or_else(|| "记忆库没开".to_owned())?;
                memory
                    .store()
                    .update_dream_settings(&memory::types::DreamSettingsPatch {
                        enabled: Some(matches!(action, CalendarActionKind::Resume)),
                        ..memory::types::DreamSettingsPatch::default()
                    })
                    .map_err(|error| error.to_string())?;
            }
            CalendarActionKind::RunNow => {
                let dream = self
                    .dream
                    .as_ref()
                    .ok_or_else(|| "做梦运行时没开".to_owned())?;
                dream.request_now()?;
            }
            CalendarActionKind::Cancel => {
                return Err("做梦是内置行，不能取消".to_owned());
            }
        }
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
            .unwrap_or_default();
        if let Err(error) = self.events.send(crate::protocol::ServiceEvent {
            seq: 0,
            time: now,
            event_type: "calendar/changed".to_owned(),
            data: serde_json::json!({ "appointmentId": "builtin:dream" }),
            cause: None,
        }) {
            eprintln!("calendar/changed 推送失败：{error}");
        }
        let appointment = self
            .calendar_builtin_view()
            .ok_or_else(|| "做梦行读不出来".to_owned())?;
        Ok(crate::protocol::CalendarActionResponse { appointment })
    }
}

impl RootHost {
    /// T077.5：任务页菜单读登记动作的事实（和 Kind 确认卡共用同一份 describe）。
    pub(crate) async fn action_describe(
        &self,
        request: crate::protocol::ActionDescribeRequest,
    ) -> Result<crate::protocol::ActionDescribeResponse, crate::error::ServiceError> {
        let kind = self.options.actions.get(&request.action).ok_or_else(|| {
            crate::error::ServiceError::TaskUnavailable(format!(
                "没有登记这个动作：{}",
                request.action
            ))
        })?;
        let details = kind
            .details(&request.details)
            .map_err(crate::error::ServiceError::TaskUnavailable)?;
        let events = self.root_session_events().await;
        let env = crate::actions::ActionEnv {
            data_dir: &self.options.data_dir,
            identity: self.options.identity.as_deref(),
            events: &events,
        };
        let facts = kind
            .describe(&details, &env)
            .map_err(crate::error::ServiceError::TaskUnavailable)?
            .into_iter()
            .map(|fact| crate::protocol::ActionFact {
                name: fact.name,
                value: fact.value,
            })
            .collect();
        Ok(crate::protocol::ActionDescribeResponse {
            action: request.action,
            facts,
        })
    }

    /// T077.5：任务页菜单确认后执行同一个登记动作。
    pub(crate) async fn action_act(
        &self,
        request: crate::protocol::ActionActRequest,
    ) -> Result<crate::protocol::ActionActResponse, crate::error::ServiceError> {
        let kind = self.options.actions.get(&request.action).ok_or_else(|| {
            crate::error::ServiceError::TaskUnavailable(format!(
                "没有登记这个动作：{}",
                request.action
            ))
        })?;
        let details = kind
            .details(&request.details)
            .map_err(crate::error::ServiceError::TaskUnavailable)?;
        let outcome = kind
            .act(&details, self)
            .await
            .map_err(crate::error::ServiceError::TaskUnavailable)?;
        Ok(crate::protocol::ActionActResponse {
            summary: outcome.summary,
            data: outcome.data,
        })
    }

    async fn root_session_events(&self) -> Vec<session::Event> {
        let guard = self.inner.lock().await;
        guard
            .as_ref()
            .map(|host| host.agent().session_events_from(0))
            .unwrap_or_default()
    }
}
