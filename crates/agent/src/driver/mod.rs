//! agent 的驱动循环与共享状态。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use model::{DeepSeek, Message, UserMessage};
use session::{AbortCause, EventKind, InboxTarget, SessionId, SessionStore};
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tools::{ToolSessionState, ToolSet};

use crate::capabilities::{
    BoxedFuture, CapabilityRegistry, CompactionPolicy, HookEvent, LoopAccess,
};
use crate::error::AgentError;
use crate::inbox::{Delivery, Inbox};
use crate::{AgentConfig, AgentEvent, CompactionTrigger, Deps};

const EVENT_CHANNEL_CAPACITY: usize = 256;
/// 服务事件面（T006.16a）：会话事件的 live 广播；容量照订阅者可能落后的距离取 1024。
const SESSION_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// 驱动阶段。
enum Phase {
    Idle,
    Running {
        abort: Arc<AtomicBool>,
        /// 中断原因：钩子/用户只在跑的一轮上写一次，收尾时取走。
        cause: Arc<Mutex<Option<AbortCause>>>,
        turn: u64,
    },
}

struct RunState {
    phase: Phase,
    wake_requested: bool,
    last_turn: u64,
}

/// 一个 step 的结束方式。
enum StepEnd {
    /// 这一步执行了工具，turn 还没结束；继续开下一步。
    Continue,
    Completed,
    MaxTokens,
    Aborted,
}

/// agent 的共享状态；驱动任务和 6 个入口方法都通过它交互。
pub(crate) struct Inner {
    id: SessionId,
    log_path: PathBuf,
    cwd: PathBuf,
    /// 会话数据目录；也是固定的用户级指令根（T005.5）。
    data_dir: PathBuf,
    /// 现在已存的工具凭证（产品装配注入；每步求值一次）。
    tool_keys: tools::ToolKeySource,

    pub(crate) model: DeepSeek,
    pub(crate) tools: ToolSet,
    /// 加载记录的状态（已经记过的工具名）；续接时从日志重建。
    pub(crate) tool_gate: Mutex<crate::tool_gate::ToolRegistryState>,
    /// 会话级工具状态（观察表/spill/任务注册表）；续接时新建，不继承上一次。
    tool_session: Arc<ToolSessionState>,
    max_parallel_tool_calls: usize,
    /// 压缩阈值比例（T005.8a）。
    pub(crate) compaction_threshold_ratio: f64,
    /// 压缩后保留的近期上下文比例（T005.8a）。
    pub(crate) compaction_retain_ratio: f64,
    /// 压缩阈值公式里的预留输出 token（T005.8a）。
    pub(crate) compaction_headroom_tokens: u64,
    /// 摘要输出 token 上限；None 时按 DSH 默认取 headroom（T006.3）。
    compaction_summary_max_tokens: Option<u32>,
    /// 仍超阈值时最多再压几次（T006.3，DSH compactionRetries）。
    compaction_retries: u32,
    /// 上下文溢出恢复最多重试几次（T006.3，DSH maxOverflowRetries）。
    compaction_max_overflow_retries: u32,
    /// 按 provider/model 覆盖压缩策略（T006.3，DSH modelPolicies）。
    compaction_model_policies: Vec<crate::CompactionModelPolicy>,
    /// 固定挂点与功能注册表（T006.1a；能力状态 per-Agent 在能力里，T006.1e）。
    pub(crate) capabilities: CapabilityRegistry,
    pub(crate) store: Mutex<SessionStore>,
    inbox: Mutex<Inbox>,
    /// 串行化 inbox 变更：保证日志里的 splice 顺序 = 内存队列的应用顺序（T006.2a）。
    inbox_mutations: tokio::sync::Mutex<()>,
    state: Mutex<RunState>,
    pub(crate) events: broadcast::Sender<AgentEvent>,
    /// T006.16a：日志事件的 live 广播（服务事件面；seq/type/data 直接来自日志事实）。
    session_events: broadcast::Sender<session::Event>,
    /// T006.F1：自引用（迟到回答的 durable 投递需要唤醒驱动）。
    self_ref: std::sync::OnceLock<std::sync::Weak<Inner>>,
    idle_notify: Notify,
    /// 取消时唤醒正在等模型流的 step（让流读取立刻停下）。
    cancel_notify: Arc<Notify>,
    /// 仅测试可见：settle_idle 预检之后、最终临界区之前的同步缝（T005.8b 边界单测）。
    #[cfg(test)]
    settle_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl LoopAccess for Inner {
    fn session(&self) -> &Mutex<SessionStore> {
        &self.store
    }
    fn events(&self) -> &broadcast::Sender<AgentEvent> {
        &self.events
    }
    fn tools(&self) -> &ToolSet {
        &self.tools
    }
    fn tool_session(&self) -> &Arc<ToolSessionState> {
        &self.tool_session
    }
    fn model(&self) -> &DeepSeek {
        &self.model
    }
    fn cwd(&self) -> &Path {
        &self.cwd
    }
    fn data_dir(&self) -> &Path {
        &self.data_dir
    }
    fn log_path(&self) -> &Path {
        &self.log_path
    }
    fn tool_keys(&self) -> std::collections::BTreeSet<tools::ToolKey> {
        self.tool_keys.present()
    }
    fn compaction_policy(&self) -> CompactionPolicy {
        let provider = self.model.provider();
        let model = self.model.config().model.as_str();
        let override_policy = self
            .compaction_model_policies
            .iter()
            .find(|policy| policy.provider == provider && policy.model == model);
        let threshold_ratio = override_policy
            .and_then(|policy| policy.threshold_ratio)
            .unwrap_or(self.compaction_threshold_ratio);
        let retain_ratio = override_policy
            .and_then(|policy| policy.retain_ratio)
            .unwrap_or(self.compaction_retain_ratio);
        let headroom_tokens = override_policy
            .and_then(|policy| policy.headroom_tokens)
            .unwrap_or(self.compaction_headroom_tokens);
        let summary_max_tokens = override_policy
            .and_then(|policy| policy.summary_max_tokens)
            .or(self.compaction_summary_max_tokens)
            .unwrap_or(u32::try_from(headroom_tokens).unwrap_or(u32::MAX));
        CompactionPolicy {
            threshold_ratio,
            retain_ratio,
            headroom_tokens,
            summary_max_tokens,
            compaction_retries: override_policy
                .and_then(|policy| policy.compaction_retries)
                .unwrap_or(self.compaction_retries),
            max_overflow_retries: override_policy
                .and_then(|policy| policy.max_overflow_retries)
                .unwrap_or(self.compaction_max_overflow_retries),
        }
    }
    fn append<'a>(&'a self, kinds: &'a [EventKind]) -> crate::capabilities::HookFuture<'a> {
        Box::pin(async move {
            Inner::append(self, kinds)
                .await
                .map(|()| crate::capabilities::HookOutcome::Continue)
        })
    }
    fn append_with_seqs<'a>(
        &'a self,
        kinds: &'a [EventKind],
    ) -> crate::capabilities::BoxedFuture<'a, Vec<u64>> {
        Box::pin(async move {
            let (seqs, before) = {
                let mut store = self.store.lock().expect("agent 会话锁被毒化");
                let before = store.events().len();
                let seqs = store.append(kinds)?;
                (seqs, before)
            };
            self.publish_appended_session_events(before);
            for kind in kinds {
                self.capabilities
                    .dispatch(&HookEvent::SessionEvent {
                        host: self,
                        event: kind,
                    })
                    .await?;
            }
            Ok(seqs)
        })
    }
    fn append_surface<'a>(
        &'a self,
        kind: EventKind,
        surface_op: session::SurfaceOp,
        source_event_seqs: Vec<u64>,
    ) -> crate::capabilities::HookFuture<'a> {
        Box::pin(async move {
            let dispatch_kind = kind.clone();
            let before = {
                let mut store = self.store.lock().expect("agent 会话锁被毒化");
                let before = store.events().len();
                store.append_surface(kind, surface_op, source_event_seqs)?;
                before
            };
            self.publish_appended_session_events(before);
            self.capabilities
                .dispatch(&HookEvent::SessionEvent {
                    host: self,
                    event: &dispatch_kind,
                })
                .await?;
            Ok(crate::capabilities::HookOutcome::Continue)
        })
    }
    fn dispatch_compaction<'a>(
        &'a self,
        turn: u64,
        step: u64,
        trigger: CompactionTrigger,
    ) -> crate::capabilities::HookFuture<'a> {
        Box::pin(async move {
            self.capabilities
                .dispatch(&HookEvent::Compaction {
                    turn,
                    step,
                    host: self,
                    trigger,
                })
                .await
                .map(|_| crate::capabilities::HookOutcome::Continue)
        })
    }

    fn current_abort(&self) -> Option<Arc<std::sync::atomic::AtomicBool>> {
        let state = self.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Running { abort, .. } => Some(Arc::clone(abort)),
            Phase::Idle => None,
        }
    }
    fn enqueue_context<'a>(&'a self, text: String) -> BoxedFuture<'a, ()> {
        Box::pin(async move {
            let target = {
                let state = self.state.lock().expect("agent 状态锁被毒化");
                match state.phase {
                    Phase::Idle => InboxTarget::NextTurn,
                    Phase::Running { .. } => InboxTarget::NextStep,
                }
            };
            let message = UserMessage::text(text).into_message();
            self.inbox_mutation(target, i64::MAX, 0, vec![message], false)
                .await?;
            Ok(())
        })
    }

    fn is_running(&self) -> bool {
        matches!(
            self.state.lock().expect("agent 状态锁被毒化").phase,
            Phase::Running { .. }
        )
    }

    fn last_turn(&self) -> u64 {
        self.state.lock().expect("agent 状态锁被毒化").last_turn
    }

    fn enqueue_message<'a>(&'a self, message: Message) -> BoxedFuture<'a, bool> {
        Box::pin(async move {
            if self
                .inbox_mutation(InboxTarget::NextStep, i64::MAX, 0, vec![message], false)
                .await
                .is_err()
            {
                return Ok(false);
            }
            if let Some(inner) = self.self_ref.get().and_then(std::sync::Weak::upgrade) {
                inner.wake_driver();
            }
            Ok(true)
        })
    }
}

impl Inner {
    pub(crate) fn start(
        config: AgentConfig,
        deps: Deps,
        last_turn: u64,
    ) -> Result<Arc<Self>, AgentError> {
        // 协调线加账：非法配置要在建 agent 时返回错误，不能等到驱动任务里 panic。
        config.validate()?;
        let Deps {
            model,
            tools,
            store,
        } = deps;
        let id = store.id().clone();
        let log_path = store.path().to_path_buf();
        let cwd = PathBuf::from(store.cwd());
        // 日志路径固定是 <data-dir>/sessions/<id>.jsonl；spill 根按会话分目录。
        let data_dir = log_path
            .parent()
            .and_then(Path::parent)
            .map_or_else(|| cwd.clone(), Path::to_path_buf);
        let tool_session = Arc::new(ToolSessionState::new(&data_dir, id.as_str()));
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let (session_events, _) = broadcast::channel(SESSION_EVENT_CHANNEL_CAPACITY);
        let instruction_roots = config.instruction_roots.clone();
        let skill_roots = config.skill_roots.clone();
        let tool_keys = config.tool_keys.clone();
        let spill_max_inline_tokens = config.spill_max_inline_tokens;
        tool_session.set_skill_roots(skill_roots.clone());
        tool_session.set_bundled_skills(config.bundled_skills.clone());
        tool_session.set_tool_keys(tool_keys.clone());
        // T006.1e：能力状态由能力模块装载/持有；driver 只做装配。
        let (instruction_state, instruction_roots) =
            crate::instructions_capability::InstructionsCapability::load(&store, instruction_roots);
        let bundled_skills = config.bundled_skills.clone();
        let (skill_catalog, skill_roots, bundled_skills) =
            crate::skill_capability::SkillCapability::load(
                &store,
                skill_roots,
                bundled_skills.clone(),
            );
        crate::todo_capability::TodoCapability::rehydrate(&tool_session, store.events());
        // T015.2：画布状态从日志折叠；重开会话后 canvas 工具接着改同一个画布。
        tool_session.set_canvases(session::canvases(store.events()));
        let reminder_chain: Arc<Mutex<Option<(String, usize)>>> = Arc::new(Mutex::new(None));
        let ask_state = crate::ask_capability::new_state();
        // T006.13（D45）：从中断修复后的日志折出仍待答的问题；回答走 Agent::answer 的迟到回答。
        crate::ask_capability::rehydrate(&ask_state, store.events());
        // T006.2a：续接/新建时从日志折叠输入队列；坏掉的 splice 让 resume 报错，不静默丢。
        let inbox = Inbox::restore(store.events())?;
        // 加载记录从日志恢复：续接以后不再重复记，工具表照旧（T014.2）。
        let tool_gate = Mutex::new(crate::tool_gate::ToolRegistryState::rehydrate(
            store.events(),
        ));
        let inner = Arc::new(Self {
            id,
            log_path,
            cwd,
            data_dir,
            tool_keys,
            model,
            tools,
            tool_gate,
            tool_session,
            max_parallel_tool_calls: config.max_parallel_tool_calls,
            compaction_threshold_ratio: config.compaction_threshold_ratio,
            compaction_retain_ratio: config.compaction_retain_ratio,
            compaction_headroom_tokens: config.compaction_headroom_tokens,
            compaction_summary_max_tokens: config.compaction_summary_max_tokens,
            compaction_retries: config.compaction_retries,
            compaction_max_overflow_retries: config.compaction_max_overflow_retries,
            compaction_model_policies: config.compaction_model_policies.clone(),
            store: Mutex::new(store),
            inbox: Mutex::new(inbox),
            inbox_mutations: tokio::sync::Mutex::new(()),
            state: Mutex::new(RunState {
                phase: Phase::Idle,
                wake_requested: false,
                last_turn,
            }),
            events,
            session_events,
            self_ref: std::sync::OnceLock::new(),
            idle_notify: Notify::new(),
            cancel_notify: Arc::new(Notify::new()),
            capabilities: CapabilityRegistry::production(crate::capabilities::CapabilityStates {
                ask: Arc::clone(&ask_state),
                instructions: Arc::clone(&instruction_state),
                skill_catalog: Arc::clone(&skill_catalog),
                reminder_chain: Arc::clone(&reminder_chain),
                instruction_roots,
                skill_roots,
                bundled_skills,
                spill_max_inline_tokens,
            }),
            #[cfg(test)]
            settle_hook: Mutex::new(None),
        });
        // T006.F1：迟到回答投递要唤醒驱动，先记下自引用。
        #[expect(
            clippy::let_underscore_must_use,
            reason = "自引用只设置一次；重复构造保留第一个弱引用"
        )]
        let _ = inner.self_ref.set(Arc::downgrade(&inner));
        // T006.1e：结算回调与完成通知投递移入 jobs_capability 模块。
        crate::jobs_capability::install(&inner);
        Ok(inner)
    }

    pub(crate) fn id(&self) -> &SessionId {
        &self.id
    }

    pub(crate) fn log_path(&self) -> &Path {
        &self.log_path
    }

    pub(crate) fn cwd(&self) -> &Path {
        &self.cwd
    }

    pub(crate) fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.events.subscribe()
    }

    /// 投递一条消息到输入队列（T006.2a 起落日志：发送返回即已收下）。
    pub(crate) async fn send(
        self: &Arc<Self>,
        message: UserMessage,
        delivery: Delivery,
    ) -> Result<(), AgentError> {
        self.send_message(message.into_message(), delivery).await
    }

    /// T006.16b：带 requestId 的发送；durable rpcId 幂等（inbox 或日志里已有同 id 就不再进）。
    pub(crate) async fn send_with_request_id(
        self: &Arc<Self>,
        request_id: &str,
        message: UserMessage,
        delivery: Delivery,
    ) -> Result<crate::SendAdmission, AgentError> {
        if self.has_request_id(request_id) {
            return Ok(crate::SendAdmission::Duplicate);
        }
        let mut message = message.into_message();
        message.source = model::MessageSource::User {
            rpc_id: Some(request_id.to_owned()),
        };
        self.send_message(message, delivery).await?;
        Ok(crate::SendAdmission::Admitted)
    }

    /// T006.16b：inbox 或已落日志的 user/message 里有没有这个 rpcId（DSH hasPromptRequest）。
    fn has_request_id(&self, request_id: &str) -> bool {
        if self
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .pending_has_rpc_id(request_id)
        {
            return true;
        }
        let store = self.store.lock().expect("agent 会话锁被毒化");
        store.events().iter().any(|event| {
            matches!(
                &event.kind,
                EventKind::UserMessage { message, .. }
                    if matches!(
                        &message.source,
                        model::MessageSource::User { rpc_id: Some(id) } if id == request_id
                    )
            )
        })
    }

    /// T006.16b：重启后领取 pending inbox（把排队的输入转成 turn）；返回是否领取了东西。
    pub(crate) fn claim_pending(self: &Arc<Self>) -> bool {
        let pending = self
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .has_pending();
        if pending {
            self.wake_driver();
        }
        pending
    }

    /// T006.16b：follow/events 的日志读面（seq > after_seq，按日志顺序）。
    pub(crate) fn session_events_from(&self, after_seq: u64) -> Vec<session::Event> {
        self.store
            .lock()
            .expect("agent 会话锁被毒化")
            .events()
            .iter()
            .filter(|event| event.seq > after_seq)
            .cloned()
            .collect()
    }

    /// T006.16b：日志最后一条事件的 seq（空日志为 0）。
    pub(crate) fn last_seq(&self) -> u64 {
        self.store
            .lock()
            .expect("agent 会话锁被毒化")
            .events()
            .last()
            .map(|event| event.seq)
            .unwrap_or(0)
    }

    /// 真正落日志的发送路径（T006.2a：先写 splice 再唤醒）。
    async fn send_message(
        self: &Arc<Self>,
        message: model::Message,
        delivery: Delivery,
    ) -> Result<(), AgentError> {
        // 新的用户消息清零重复调用链（照 DSH repeat-tool-reminder；状态在提醒能力里）。
        self.capabilities.reset_repeat_chain();
        let target = {
            let state = self.state.lock().expect("agent 状态锁被毒化");
            let aborting = match &state.phase {
                Phase::Running { abort, .. } => abort.load(Ordering::SeqCst),
                Phase::Idle => false,
            };
            // 已经取消的活动收不下会唤醒的输入：改投下一轮（照 DSH 的 send）。
            if aborting && delivery.wakes() {
                InboxTarget::NextTurn
            } else {
                delivery.target()
            }
        };
        self.inbox_mutation(target, i64::MAX, 0, vec![message], false)
            .await?;
        if delivery.wakes() {
            self.wake_driver();
        }
        Ok(())
    }

    /// 领取一个 step 的输入（T006.2a：从日志恢复的队列上领取，领取也落日志）。
    ///
    /// 照 DSH claim（inbox.ts:108-113）：先领全部 next-step，target=next-turn 时再领
    /// 一条 next-turn；claimed 通知带 owning turn。
    pub(crate) async fn claim_input(
        &self,
        target: InboxTarget,
        turn: u64,
    ) -> Result<Vec<Message>, AgentError> {
        let _gate = self.inbox_mutations.lock().await;
        let mut claimed = self
            .inbox_splice(InboxTarget::NextStep, 0, i64::MAX, Vec::new(), false)
            .await?;
        if target == InboxTarget::NextTurn {
            claimed.extend(
                self.inbox_splice(InboxTarget::NextTurn, 0, 1, Vec::new(), false)
                    .await?,
            );
        }
        for message in &claimed {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "事件订阅端未连接时丢弃副本；入队事实已落盘"
            )]
            let _ = self.events.send(AgentEvent::InboxClaimed {
                message: message.clone(),
                turn,
            });
        }
        Ok(claimed)
    }

    /// 取消：清空输入队列并中止正在跑的一轮（T006.2a 起取消也落日志）。
    pub(crate) async fn cancel(&self) -> Result<(), AgentError> {
        // 照 DSH clear（inbox.ts:96-100）：先清 next-step、再清 next-turn，普通删除带 canceled。
        {
            let _gate = self.inbox_mutations.lock().await;
            self.inbox_splice(InboxTarget::NextStep, 0, i64::MAX, Vec::new(), true)
                .await?;
            self.inbox_splice(InboxTarget::NextTurn, 0, i64::MAX, Vec::new(), true)
                .await?;
        }
        self.cancel_with(AbortCause::User);
        Ok(())
    }

    /// 取消当前一轮，不动输入队列（钩子取消用）；cause 写进 turn/end。
    pub(crate) fn cancel_with(&self, cause: AbortCause) {
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        state.wake_requested = false;
        if let Phase::Running {
            abort, cause: slot, ..
        } = &state.phase
        {
            *slot.lock().expect("agent 中断原因锁被毒化") = Some(cause);
            abort.store(true, Ordering::SeqCst);
            self.cancel_notify.notify_waiters();
            self.capabilities.cancel_questions();
        }
    }

    /// 规范化 + 落日志 + 应用 + 发通知；调用方必须持有 `inbox_mutations` 锁。
    async fn inbox_splice(
        &self,
        target: InboxTarget,
        start: i64,
        delete_count: i64,
        inserted: Vec<Message>,
        discard_removed: bool,
    ) -> Result<Vec<Message>, AgentError> {
        let splice = {
            let inbox = self.inbox.lock().expect("agent 收件箱锁被毒化");
            inbox.splice(target, start, delete_count, inserted, discard_removed)
        };
        if splice.is_empty() {
            return Ok(Vec::new());
        }
        // 重复 MessageId 的 splice 不落盘、不改内存（DSH inbox.ts:41-48；live 路径共用
        // resume 的同一查重逻辑，防止损坏的调用方把同一条消息插两遍）。
        self.inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .validate(&splice)?;
        // 先落盘再改内存：日志里的 splice 顺序就是队列的应用顺序；写失败时队列不动。
        self.append(&[splice.event_kind()]).await?;
        self.inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .apply(&splice);
        if splice.discard_removed() {
            for message in splice.removed() {
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "事件订阅端未连接时丢弃副本；入队事实已落盘"
                )]
                let _ = self.events.send(AgentEvent::InboxDiscarded {
                    message: message.clone(),
                });
            }
        }
        for message in splice.inserted() {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "事件订阅端未连接时丢弃副本；入队事实已落盘"
            )]
            let _ = self.events.send(AgentEvent::InboxInserted {
                message: message.clone(),
            });
        }
        Ok(splice.removed().to_vec())
    }

    /// inbox 变更入口：持 `inbox_mutations` 锁串行化（send / cancel / enqueue_context）。
    async fn inbox_mutation(
        &self,
        target: InboxTarget,
        start: i64,
        delete_count: i64,
        inserted: Vec<Message>,
        discard_removed: bool,
    ) -> Result<Vec<Message>, AgentError> {
        let _gate = self.inbox_mutations.lock().await;
        self.inbox_splice(target, start, delete_count, inserted, discard_removed)
            .await
    }

    /// 一次 step 的工具批之后推进重复链，命中阈值（3/5/8，照 DSH）返回一条提醒文本。
    /// todo_write 不跟踪（DSH 基础组合包默认排除）；每步最多一条提醒。
    /// T005.10/T006.1d2：回答问题；question_id 必须匹配当前等待的问题且未答过。
    /// T006.13：答案照 DSH 形状（selected 选项标签 + 可选 custom 文本）。等待中的批由 hook
    /// 收尾；重启恢复的批整批答完后写 question/answered 并投递 DSH 的迟到回答消息。
    pub(crate) async fn answer(
        self: &Arc<Self>,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> bool {
        self.capabilities
            .answer(self.as_ref(), question_id, selected, custom)
            .await
    }

    /// T006.13：撤销一条待答问题（提问服务路径；恢复批的事件编排在 ask 能力里）。
    pub(crate) async fn cancel_question(&self, question_id: &str) -> bool {
        self.capabilities.cancel_question(self, question_id).await
    }

    /// T006.13：当前待答问题（含重启恢复）。
    pub(crate) fn pending_questions(&self) -> Vec<crate::Question> {
        self.capabilities.pending_questions()
    }

    /// T006.1e：jobs_capability 安装结算回调用（任务注册表在 tools 层）。
    pub(crate) fn job_session(&self) -> &Arc<ToolSessionState> {
        &self.tool_session
    }

    /// T006.1e：结算回调回到 runtime 上下文后唤醒驱动。
    pub(crate) fn wake(self: &Arc<Self>) {
        self.wake_driver();
    }
    /// 起驱动或在 Running 时置 latch。
    ///
    /// T004.1 审计 known-weakness 3：收尾检查与 settle 快照之间来的 FollowUp/Steer
    /// 过去会被丢掉。Running 时无条件 latch，settle 看到 wake && pending 会重开驱动。
    fn wake_driver(self: &Arc<Self>) {
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Idle => {
                let turn = state.last_turn;
                state.phase = Phase::Running {
                    abort: Arc::new(AtomicBool::new(false)),
                    cause: Arc::new(Mutex::new(None)),
                    turn,
                };
                drop(state);
                tokio::spawn(kick(Arc::clone(self)));
            }
            Phase::Running { .. } => {
                state.wake_requested = true;
            }
        }
    }

    pub(crate) async fn idle(&self) {
        loop {
            let notified = self.idle_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if matches!(
                &self.state.lock().expect("agent 状态锁被毒化").phase,
                Phase::Idle
            ) {
                return;
            }
            notified.await;
        }
    }

    /// 写日志并派发 SessionEvent 挂点（T006.1a：日志先落盘，钩子随后执行；
    /// 钩子返回 Err 时日志已提交，调用方按错误收尾）。
    pub(crate) async fn append(&self, kinds: &[EventKind]) -> Result<(), AgentError> {
        let before = {
            let mut store = self.store.lock().expect("agent 会话锁被毒化");
            let before = store.events().len();
            store.append(kinds)?;
            before
        };
        self.publish_appended_session_events(before);
        for kind in kinds {
            self.capabilities
                .dispatch(&HookEvent::SessionEvent {
                    host: self,
                    event: kind,
                })
                .await?;
        }
        Ok(())
    }

    /// T006.16a：把 [before] 之后新落的日志事件广播到服务事件面（seq/type/data 都来自日志事实）。
    ///
    /// 订阅者落后由容量兜底（Lagged 时按 DSH 的 follow 语义重新跟随，见 T006.16b）。
    fn publish_appended_session_events(&self, before: usize) {
        let store = self.store.lock().expect("agent 会话锁被毒化");
        let events = store.events();
        for event in events.iter().skip(before.min(events.len())) {
            #[expect(
                clippy::let_underscore_must_use,
                reason = "服务事件面没有订阅者时广播失败；日志事实已持久化"
            )]
            let _ = self.session_events.send(event.clone());
        }
    }

    /// T006.16a：服务事件面的订阅端（服务宿主转发给载体）。
    pub(crate) fn subscribe_session_events(&self) -> broadcast::Receiver<session::Event> {
        self.session_events.subscribe()
    }

    /// T006.16a：被动状态用的运行态（有正在跑的轮次）。
    pub(crate) fn is_running(&self) -> bool {
        matches!(
            self.state.lock().expect("agent 状态锁被毒化").phase,
            Phase::Running { .. }
        )
    }

    /// 手动 /compact（T006.F1）：入口在 compaction 能力，driver 只派发（DoD1）。
    pub(crate) async fn compact_manual(
        self: &Arc<Self>,
        source_command_id: Option<String>,
    ) -> Result<Option<crate::CompactionOutcome>, AgentError> {
        self.capabilities
            .compact_manual(self.as_ref(), source_command_id)
            .await
    }
}

/// 驱动任务：连续跑若干轮，直到收件箱里没有会唤醒的输入。
mod helpers;
mod retry;
mod step;
mod turn;

use turn::*;
