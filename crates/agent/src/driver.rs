//! agent 的驱动循环与共享状态。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use model::{
    AssistantStreamAccumulator, BlockType, CancelToken, ContentBlock, DeepSeek, ErrorCode,
    FinishReason, Message, ModelError, StreamChunk, TimedStreamChunk, Usage, UserMessage,
};
use session::{
    AbortCause, EventKind, InboxTarget, LlmFailure, RetryId, RetryMode, SessionId, SessionStore,
    ToolError, TurnEndReason,
};
use tokio::sync::Notify;
use tokio::sync::broadcast;
use tools::{ToolCall, ToolContext, ToolSessionState, ToolSet, execute_batch};

use crate::ask_capability::{AnswerOutcome, CancelOutcome, RestoredReply};
use crate::capabilities::{
    BoxedFuture, CapabilityRegistry, CompactionPolicy, HookEvent, LoopAccess,
};
use crate::error::AgentError;
use crate::inbox::{Delivery, Inbox};
use crate::{AgentConfig, AgentEvent, CompactionTrigger, Deps, TurnOutcome};

const EVENT_CHANNEL_CAPACITY: usize = 256;
/// 服务事件面（T006.16a）：会话事件的 live 广播；容量照订阅者可能落后的距离取 1024。
const SESSION_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// 驱动阶段。
enum Phase {
    Idle,
    Running { abort: Arc<AtomicBool>, turn: u64 },
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

    pub(crate) model: DeepSeek,
    pub(crate) tools: ToolSet,
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
        let spill_max_inline_tokens = config.spill_max_inline_tokens;
        tool_session.set_skill_roots(skill_roots.clone());
        // T006.1e：能力状态由能力模块装载/持有；driver 只做装配。
        let (instruction_state, instruction_roots) =
            crate::instructions_capability::InstructionsCapability::load(&store, instruction_roots);
        let (skill_catalog, skill_roots) =
            crate::skill_capability::SkillCapability::load(&store, skill_roots);
        crate::todo_capability::TodoCapability::rehydrate(&tool_session, store.events());
        let reminder_chain: Arc<Mutex<Option<(String, usize)>>> = Arc::new(Mutex::new(None));
        let compaction_waiters: Arc<Mutex<Vec<crate::capabilities::ManualCompaction>>> =
            Arc::new(Mutex::new(Vec::new()));
        let ask_state = crate::ask_capability::new_state();
        // T006.13（D45）：从中断修复后的日志折出仍待答的问题；回答走 Agent::answer 的迟到回答。
        crate::ask_capability::rehydrate(&ask_state, store.events());
        // T006.2a：续接/新建时从日志折叠输入队列；坏掉的 splice 让 resume 报错，不静默丢。
        let inbox = Inbox::restore(store.events())?;
        let inner = Arc::new(Self {
            id,
            log_path,
            cwd,
            data_dir,
            model,
            tools,
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
            idle_notify: Notify::new(),
            cancel_notify: Arc::new(Notify::new()),
            capabilities: CapabilityRegistry::production(crate::capabilities::CapabilityStates {
                ask: Arc::clone(&ask_state),
                instructions: Arc::clone(&instruction_state),
                skill_catalog: Arc::clone(&skill_catalog),
                reminder_chain: Arc::clone(&reminder_chain),
                compaction_waiters: Arc::clone(&compaction_waiters),
                instruction_roots,
                skill_roots,
                spill_max_inline_tokens,
            }),
            #[cfg(test)]
            settle_hook: Mutex::new(None),
        });
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
        let mut state = self.state.lock().expect("agent 状态锁被毒化");
        state.wake_requested = false;
        if let Phase::Running { abort, .. } = &state.phase {
            abort.store(true, Ordering::SeqCst);
            // 让正在等模型流的 step 立刻醒来、走 interrupted 收尾。
            self.cancel_notify.notify_waiters();
            // T006.1d2：让正在等用户回答的 ask 能力醒来，撤销问题并 aborted 收尾。
            self.capabilities.cancel_questions();
        }
        Ok(())
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
                let _ = self.events.send(AgentEvent::InboxDiscarded {
                    message: message.clone(),
                });
            }
        }
        for message in splice.inserted() {
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
        match self
            .capabilities
            .answer_question(question_id, selected, custom)
        {
            AnswerOutcome::Rejected => false,
            AnswerOutcome::Accepted => true,
            AnswerOutcome::RestoredReply(reply) => self.deliver_restored_reply(reply).await,
        }
    }

    /// T006.13：把恢复问题的整批回答写成 durable 事实，再以 DSH 的迟到回答消息接续。
    async fn deliver_restored_reply(self: &Arc<Self>, reply: RestoredReply) -> bool {
        // 先落 question/answered 再投递：答案先成为日志事实，模型下一轮再从 user 消息看到它。
        // 落盘失败返回 false（AskState 里该批已结算；已知边界见 handback §9）。
        let events: Vec<EventKind> = reply
            .answers
            .iter()
            .map(|(question_id, answer)| EventKind::QuestionAnswered {
                turn: reply.turn,
                step: reply.step,
                call_id: reply.call_id.clone(),
                question_id: question_id.clone(),
                selected: answer.selected.clone(),
                custom: answer.custom.clone(),
            })
            .collect();
        if self.append(&events).await.is_err() {
            return false;
        }
        let message = crate::ask_capability::late_reply_message(&reply);
        if self
            .inbox_mutation(InboxTarget::NextStep, i64::MAX, 0, vec![message], false)
            .await
            .is_err()
        {
            return false;
        }
        self.wake_driver();
        true
    }

    /// T006.13：撤销一条待答问题（提问服务路径）。等待中的批由 hook 以 ASK_ABORTED 收尾；
    /// 恢复的批移除并写 question/cancelled（内存里已收的答案先补成 question/answered）。
    pub(crate) async fn cancel_question(&self, question_id: &str) -> bool {
        match self.capabilities.cancel_question(question_id) {
            CancelOutcome::Rejected => false,
            CancelOutcome::Live => true,
            CancelOutcome::Restored(cancel) => {
                let mut events: Vec<EventKind> = cancel
                    .answered
                    .iter()
                    .map(|(question_id, answer)| EventKind::QuestionAnswered {
                        turn: cancel.turn,
                        step: cancel.step,
                        call_id: cancel.call_id.clone(),
                        question_id: question_id.clone(),
                        selected: answer.selected.clone(),
                        custom: answer.custom.clone(),
                    })
                    .collect();
                events.extend(cancel.cancelled.iter().map(|question_id| {
                    EventKind::QuestionCancelled {
                        turn: cancel.turn,
                        step: cancel.step,
                        call_id: cancel.call_id.clone(),
                        question_id: question_id.clone(),
                    }
                }));
                self.append(&events).await.is_ok()
            }
        }
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

    /// 请求一次手动压缩（T005.8b/T006.3）：排队并唤醒驱动；接收者拿到结果计数。
    pub(crate) fn queue_compaction(
        self: &Arc<Self>,
        source_command_id: Option<String>,
    ) -> tokio::sync::oneshot::Receiver<
        Result<Option<crate::CompactionOutcome>, crate::compaction::ManualCompactionFailure>,
    > {
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let request = crate::capabilities::ManualCompaction {
            source_command_id,
            reply,
        };
        if self.capabilities.push_compaction_waiter(request).is_err() {
            // 没有能力接收：接收端会立刻拿到 RecvError，Agent::compact 转成错误。
        }
        self.wake_driver();
        receiver
    }

    /// 队列里还有没有等待手动压缩的请求。
    fn has_queued_compaction(&self) -> bool {
        self.capabilities.has_compaction_waiters()
    }

    /// kick 在轮次结束后处理排队的手动压缩；空闲时也会被 wake 进来处理。
    /// 语义：同一批排队的请求合并成一次压缩（所有等待者一起唤醒）；压缩进行中新到的
    /// 请求会进入下一轮 drain，再做一次压缩。循环 drain 保证不会丢等待者。
    async fn run_queued_compaction(self: &Arc<Self>) -> Result<(), AgentError> {
        loop {
            let waiters = self.capabilities.take_compaction_waiters();
            if waiters.is_empty() {
                return Ok(());
            }
            let turn = {
                let state = self.state.lock().expect("agent 状态锁被毒化");
                state.last_turn
            };
            // T006.3：手动 /compact 直接走 compact_now（两次 turn 之间独立压缩，不带
            // PreStep 的注入路径）；同一批等待的请求合并成一次压缩，结果广播给每个等待者。
            // 合并时只保留第一个 sourceCommandId（事件字段只能带一个发起命令）。
            let source_command_id = waiters
                .iter()
                .find_map(|request| request.source_command_id.clone());
            let result =
                crate::compaction::compact_now(self.as_ref(), turn, source_command_id).await;
            let reply = match result {
                Ok(outcome) => Ok(outcome),
                Err(error) => Err(crate::compaction::manual_failure(error)),
            };
            for request in waiters {
                let _ = request.reply.send(reply.clone());
            }
        }
    }
}

/// 驱动任务：连续跑若干轮，直到收件箱里没有会唤醒的输入。
async fn kick(inner: Arc<Inner>) {
    loop {
        // T005.8b：手动压缩排在当前轮之后；空闲时先处理队列，避免为它造空 turn。
        if inner.has_queued_compaction() {
            let _ = inner.run_queued_compaction().await;
            continue;
        }
        let has_input = inner
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .has_pending();
        if !has_input {
            break;
        }
        if !matches!(run_turn(&inner).await, Ok(true)) {
            break;
        }
    }
    let status_turn = inner.state.lock().expect("agent 状态锁被毒化").last_turn;
    if let Err(error) = inner
        .capabilities
        .dispatch(&HookEvent::Status {
            turn: status_turn,
            host: inner.as_ref(),
        })
        .await
    {
        let _ = error;
    }
    settle_idle(&inner);
}

/// 收敛到 Idle；如果取消之后有人唤过，就立刻再起一个驱动。
fn settle_idle(inner: &Arc<Inner>) {
    let (turn, wake) = {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        let turn = match &state.phase {
            Phase::Running { turn, .. } => *turn,
            Phase::Idle => state.last_turn,
        };
        state.last_turn = turn;
        let wake = std::mem::take(&mut state.wake_requested);
        (turn, wake)
    };
    let has_queued = inner.has_queued_compaction();
    let has_pending = inner
        .inbox
        .lock()
        .expect("agent 收件箱锁被毒化")
        .has_pending();
    if has_queued || (wake && has_pending) {
        restart_kick(inner, turn);
        return;
    }
    // 仅测试可见的同步缝：让边界单测能精确停在「预检完成、最终临界区之前」。
    #[cfg(test)]
    {
        let hook = inner
            .settle_hook
            .lock()
            .expect("settle hook 锁被毒化")
            .clone();
        if let Some(hook) = hook {
            hook();
        }
    }
    // 收尾临界区：在同一把 state 锁里重查一次 wake/队列再转 Idle。
    // 否则 compact 可能在 has_queued 之后、Idle 之前入队：queue_compaction 的
    // wake_driver 看到 Running 只置 wake_requested，settle 随后设 Idle 就丢了 waiter。
    let mut state = inner.state.lock().expect("agent 状态锁被毒化");
    let wake_now = std::mem::take(&mut state.wake_requested);
    let queued_now = inner.has_queued_compaction();
    if wake_now || queued_now {
        drop(state);
        restart_kick(inner, turn);
        return;
    }
    state.phase = Phase::Idle;
    drop(state);
    inner.idle_notify.notify_waiters();
}

fn restart_kick(inner: &Arc<Inner>, turn: u64) {
    let mut state = inner.state.lock().expect("agent 状态锁被毒化");
    state.phase = Phase::Running {
        abort: Arc::new(AtomicBool::new(false)),
        turn,
    };
    drop(state);
    tokio::spawn(kick(Arc::clone(inner)));
}

async fn run_turn(inner: &Arc<Inner>) -> Result<bool, AgentError> {
    let (current_turn, abort) = {
        let state = inner.state.lock().expect("agent 状态锁被毒化");
        match &state.phase {
            Phase::Running { abort, turn } => (*turn, Arc::clone(abort)),
            Phase::Idle => return Ok(false),
        }
    };
    // step 是这一轮内的局部计数：每轮从 1 开始，不跨 turn 携带（T004 审 K1）。
    let mut step = 0u64;
    let turn = current_turn + 1;
    inner.append(&[EventKind::TurnStart { turn }]).await?;
    {
        let mut state = inner.state.lock().expect("agent 状态锁被毒化");
        if let Phase::Running { turn: current, .. } = &mut state.phase {
            *current = turn;
        }
    }

    let mut turn_end: Option<TurnEndReason> = None;
    let steps = run_turn_steps(inner, turn, &mut step, &abort, &mut turn_end).await;

    let (reason, outcome) = match &steps {
        Ok(()) => {
            let reason = turn_end.unwrap_or_else(|| TurnEndReason::Error {
                error: LlmFailure {
                    code: "UNKNOWN".to_owned(),
                    message: "turn ended without a reason".to_owned(),
                },
            });
            let outcome = TurnOutcome::from(&reason);
            (reason, outcome)
        }
        Err(error) => {
            let reason = TurnEndReason::Error {
                error: LlmFailure {
                    code: error.code(),
                    message: error.to_string(),
                },
            };
            let outcome = TurnOutcome::from(&reason);
            (reason, outcome)
        }
    };
    inner
        .capabilities
        .dispatch(&HookEvent::TurnStopping {
            turn,
            host: inner.as_ref(),
        })
        .await?;
    let appended = inner.append(&[EventKind::TurnEnd { turn, reason }]).await;
    let _ = inner.events.send(AgentEvent::TurnEnded { turn, outcome });
    appended?;
    let mut result = match steps {
        Ok(()) => Ok(inner
            .inbox
            .lock()
            .expect("agent 收件箱锁被毒化")
            .has_pending()),
        Err(error) => Err(error),
    };
    // T004.3 审计 fix-now 1：取消之后、收尾之前投递的消息会被 latch；若把
    // has_pending 带回 kick，就会用同一个已 abort 的 Phase 空转。按「abort 置位」
    // （不看 reason：cancel 也可能落在正常完成之后）收敛到 settle_idle；settle 会
    // 用新 Phase（新 abort）重开驱动，领取取消后投递的输入。
    if abort.load(Ordering::SeqCst) {
        result = Ok(false);
    }
    // 收尾检查与 settle_idle 的 wake_requested 快照之间只有一个显式让出点
    // （tokio::task::yield_now），除此之外没有别的 await；让排队中的
    // FollowUp/Steer 能在 settle 之前落地（确定性回归靠这个点，不靠 sleep）。
    tokio::task::yield_now().await;
    result
}

/// 一轮里不断开 step：turn 开始时领取一条 follow-up，加上全部 next-step；
/// 之后的 step 只领取 next-step。执行了工具、轮次还没结束时继续开下一步。
async fn run_turn_steps(
    inner: &Arc<Inner>,
    turn: u64,
    step: &mut u64,
    abort: &Arc<AtomicBool>,
    turn_end: &mut Option<TurnEndReason>,
) -> Result<(), AgentError> {
    let mut target = InboxTarget::NextTurn;
    loop {
        if abort.load(Ordering::SeqCst) {
            *turn_end = Some(TurnEndReason::Aborted {
                reason: AbortCause::User,
            });
            return Ok(());
        }
        let claimed = inner.claim_input(target, turn).await?;
        if turn_end.is_some() && claimed.is_empty() {
            return Ok(());
        }
        if *step == 0 && claimed.is_empty() {
            *turn_end = Some(TurnEndReason::Completed);
            return Ok(());
        }
        *step += 1;
        let step_number = *step;
        inner
            .append(&[EventKind::StepStart {
                turn,
                step: step_number,
            }])
            .await?;
        let outcome = run_step(inner, turn, step_number, claimed, abort).await;
        let end = match outcome {
            Ok(end) => end,
            Err(error) => {
                if let Err(cleanup_error) = inner
                    .append(&[EventKind::StepEnd {
                        turn,
                        step: step_number,
                    }])
                    .await
                {
                    return Err(AgentError::StepCleanup {
                        first: error.to_string(),
                        second: cleanup_error.to_string(),
                    });
                }
                return Err(error);
            }
        };
        inner
            .append(&[EventKind::StepEnd {
                turn,
                step: step_number,
            }])
            .await?;
        match end {
            StepEnd::Continue => {}
            StepEnd::Completed => {
                if !matches!(turn_end.as_ref(), Some(TurnEndReason::MaxTokens)) {
                    *turn_end = Some(TurnEndReason::Completed);
                }
            }
            StepEnd::MaxTokens => {
                if !matches!(turn_end.as_ref(), Some(TurnEndReason::MaxTokens)) {
                    *turn_end = Some(TurnEndReason::MaxTokens);
                }
            }
            StepEnd::Aborted => {
                *turn_end = Some(TurnEndReason::Aborted {
                    reason: AbortCause::User,
                });
            }
        }
        if turn_end.is_some()
            && inner
                .inbox
                .lock()
                .expect("agent 收件箱锁被毒化")
                .next_step_is_empty()
        {
            return Ok(());
        }
        target = InboxTarget::NextStep;
    }
}

/// 一个 step：写 system/user 事件，从日志派生请求，流式收模型输出，
/// 有工具调用就派发批执行并写 tool/call、tool/result，否则收尾这一轮。
async fn run_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    claimed: Vec<Message>,
    abort: &Arc<AtomicBool>,
) -> Result<StepEnd, AgentError> {
    if abort.load(Ordering::SeqCst) {
        return Ok(StepEnd::Aborted);
    }
    let has_system = inner
        .store
        .lock()
        .expect("agent 会话锁被毒化")
        .has_system_message();
    if !has_system {
        let message = Message::system_prompt(system_prompt(&inner.cwd, &inner.tools));
        inner
            .append(&[EventKind::SystemMessage {
                turn,
                step,
                message,
            }])
            .await?;
    }
    let claimed_messages = claimed.clone();
    for message in claimed {
        inner
            .append(&[EventKind::UserMessage {
                turn,
                step,
                message,
            }])
            .await?;
    }

    // T006.1c：PreStep 两阶段——Inject（instructions/skill 注入）在旧位置，再派发 Compact
    // （压缩自动检查）。prune 已挂到 Compaction 挂点（T006.4），不再每请求剪、也不参与
    // PreStep 顺序；真正的修剪只发生在压缩事务里。
    inner
        .capabilities
        .dispatch(&HookEvent::PreStep {
            turn,
            step,
            host: inner.as_ref(),
            messages: &claimed_messages,
            trigger: None,
            phase: crate::capabilities::PreStepPhase::Inject,
        })
        .await?;
    // T005.8a：请求前检查 token 压力；越过阈值就压缩较早历史（失败不打断本轮，事件已记录）。
    // T006.1b：压缩实现已移出 loop，这里只派发固定挂点；压缩能力在 should_trigger/
    // select_split 确认后才会真正压缩，否则 no-op。
    inner
        .capabilities
        .dispatch(&HookEvent::PreStep {
            turn,
            step,
            host: inner.as_ref(),
            messages: &claimed_messages,
            trigger: None,
            phase: crate::capabilities::PreStepPhase::Compact,
        })
        .await?;
    let mut messages = inner
        .store
        .lock()
        .expect("agent 会话锁被毒化")
        .derive_messages();
    let tool_schemas = inner.tools.definitions();
    // 取消令牌共用 agent 的 abort 标志 + Notify：等响应头阶段也能被打断。
    let cancel = CancelToken::new(Arc::clone(abort), Arc::clone(&inner.cancel_notify));
    // 已完成的块按 index 存；打开中的 text/reasoning 块累计增量，取消时按 DSH
    // interruptedBlocks 的规则拼出可安全收尾的前缀（text/reasoning、去空白非空、
    // tool-call 丢弃）。
    let mut blocks: BTreeMap<usize, ContentBlock> = BTreeMap::new();
    let mut open: BTreeMap<usize, (BlockType, String)> = BTreeMap::new();
    let mut usage: Option<Usage> = None;
    let mut finish: Option<FinishReason> = None;
    // 同一 step 的重试链 id（首个 llm/retry 生成，之后复用；DSH 投影在 step/start、turn/end 后重置）。
    let mut retry_id: Option<RetryId> = None;
    // 每次模型尝试的精确帧记录（成功写进 assistant/message，失败写进 assistant/attempt；
    // DSH 的 AssistantStreamAttempt，agent-loop/src/assistant-stream.ts:18-43）。
    let mut attempt_stream = AssistantStreamAccumulator::new();
    let mut attempt: u32 = 0;
    let mut overflow_retries = 0_u32;
    'attempts: loop {
        if abort.load(Ordering::SeqCst) {
            return Ok(StepEnd::Aborted);
        }
        inner
            .capabilities
            .dispatch(&HookEvent::Request {
                turn,
                step,
                host: inner.as_ref(),
                messages: &messages,
            })
            .await?;
        let mut stream = match inner.model.stream(&messages, &tool_schemas, &cancel).await {
            Ok(stream) => stream,
            Err(error) => {
                if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                    // 请求还没开始流出（DSH agent.ts:488-500：started=false 不落 attempt）。
                    return Ok(StepEnd::Aborted);
                }
                // T005.8a：上下文溢出先压缩再重试一次；压缩不了或第二次溢出按正常错误收尾。
                // T006.1b：压缩由 RequestError 挂点里的压缩能力执行；它压缩不了时返回
                // 原始错误，这里 `?` 直接收尾（行为与旧路径一致）。
                if error.code == ErrorCode::ContextWindowExceeded {
                    // DSH maxOverflowRetries（默认 1）：每次溢出前压缩恢复；次数用尽后
                    // 按原错误收尾，不再派发/压缩（防无限循环）。
                    let max_overflow_retries = inner.compaction_policy().max_overflow_retries;
                    if overflow_retries < max_overflow_retries {
                        overflow_retries += 1;
                        inner
                            .capabilities
                            .dispatch(&HookEvent::RequestError {
                                turn,
                                step,
                                host: inner.as_ref(),
                                error: &error,
                            })
                            .await?;
                        attempt_stream = AssistantStreamAccumulator::new();
                        messages = inner
                            .store
                            .lock()
                            .expect("agent 会话锁被毒化")
                            .derive_messages();
                        continue 'attempts;
                    }
                    return Err(AgentError::Model(error));
                }
                // 请求还没流出（DSH started=false），这次不算 attempt：不落 assistant/attempt。
                inner
                    .capabilities
                    .dispatch(&HookEvent::RequestError {
                        turn,
                        step,
                        host: inner.as_ref(),
                        error: &error,
                    })
                    .await?;
                match decide_retry(
                    inner,
                    &cancel,
                    &error,
                    turn,
                    step,
                    &mut attempt,
                    &mut retry_id,
                )
                .await?
                {
                    RetryDecision::Retry => {
                        reset_attempt(
                            &mut blocks,
                            &mut open,
                            &mut usage,
                            &mut finish,
                            &mut attempt_stream,
                        );
                        continue 'attempts;
                    }
                    RetryDecision::Cancel => return Ok(StepEnd::Aborted),
                    RetryDecision::Fatal => return Err(AgentError::Model(error)),
                }
            }
        };
        loop {
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
            }
            let notified = inner.cancel_notify.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
            }
            let chunk = tokio::select! {
                chunk = stream.next() => chunk,
                () = &mut notified => {
                    return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
                }
            };
            let Some(chunk) = chunk else {
                break;
            };
            if let Ok(chunk) = &chunk {
                attempt_stream.push(TimedStreamChunk {
                    time: now_ms(),
                    chunk: chunk.clone(),
                });
            }
            match chunk {
                Ok(StreamChunk::BlockStart { index, block_type }) => {
                    if matches!(block_type, BlockType::Text | BlockType::Reasoning) {
                        open.insert(index, (block_type, String::new()));
                    }
                }
                Ok(StreamChunk::TextDelta { index, text }) => {
                    if let Some((_, buffer)) = open.get_mut(&index) {
                        buffer.push_str(&text);
                    }
                    let _ = inner.events.send(AgentEvent::AssistantTextDelta { text });
                }
                Ok(StreamChunk::ReasoningDelta { index, text }) => {
                    if let Some((_, buffer)) = open.get_mut(&index) {
                        buffer.push_str(&text);
                    }
                }
                // 工具参数增量只进 stream 记录（T006.14 照 DSH tool-call-delta）；
                // 完整参数在 block-end 的 block 里，不需要额外状态。
                Ok(StreamChunk::ToolCallDelta { .. }) => {}
                Ok(StreamChunk::BlockEnd { index, block }) => {
                    open.remove(&index);
                    blocks.insert(index, block);
                }
                // K10：适配器报的整段用量为空时当作没报（不伪造空 usage）。
                Ok(StreamChunk::Usage { usage: reported }) => {
                    if !reported.is_empty() {
                        usage = Some(reported);
                    }
                }
                Ok(StreamChunk::Finish { reason }) => finish = Some(reason),
                Err(error) => {
                    if abort.load(Ordering::SeqCst) || error.code == ErrorCode::Aborted {
                        return interrupted_step(
                            inner,
                            turn,
                            step,
                            &blocks,
                            &open,
                            &usage,
                            &attempt_stream,
                        )
                        .await;
                    }
                    // 流已经开过：这次尝试没有可见消息，照 DSH agent.ts:567-580 先落
                    // assistant/attempt（帧保留），再决定重试/收尾。
                    append_attempt(inner, turn, step, &attempt_stream).await?;
                    match decide_retry(
                        inner,
                        &cancel,
                        &error,
                        turn,
                        step,
                        &mut attempt,
                        &mut retry_id,
                    )
                    .await?
                    {
                        RetryDecision::Retry => {
                            reset_attempt(
                                &mut blocks,
                                &mut open,
                                &mut usage,
                                &mut finish,
                                &mut attempt_stream,
                            );
                            continue 'attempts;
                        }
                        RetryDecision::Cancel => return Ok(StepEnd::Aborted),
                        RetryDecision::Fatal => return Err(AgentError::Model(error)),
                    }
                }
            }
            if abort.load(Ordering::SeqCst) {
                return interrupted_step(
                    inner,
                    turn,
                    step,
                    &blocks,
                    &open,
                    &usage,
                    &attempt_stream,
                )
                .await;
            }
        }
        break 'attempts;
    }
    // 流已收完但取消在结算前到了：照 DSH 的 for-await 后 throwIfAborted 走中断前缀。
    if abort.load(Ordering::SeqCst) {
        return interrupted_step(inner, turn, step, &blocks, &open, &usage, &attempt_stream).await;
    }
    let Some(finish) = finish else {
        return Err(AgentError::Model(ModelError::new(
            model::ErrorCode::StreamClosed,
            "模型流没有给出结束原因",
        )));
    };
    // max-tokens 时裁掉被截断的 tool-call 块（照 DSH assembler）：日志里不能留没有
    // 配对 tool/result 的 tool_use；text/reasoning 保留。
    let content: Vec<ContentBlock> = blocks
        .values()
        .filter(|block| {
            !(finish == FinishReason::MaxTokens && matches!(block, ContentBlock::ToolCall { .. }))
        })
        .cloned()
        .collect();
    // 成功结算一律写 assistant/message（DSH agent.ts:566-584）：content 裁空也要写，
    // 让 usage 落日志（T004 K9）；派生历史会跳过空消息（session/surface.ts:140）。
    let message = assistant_message(inner, content.clone());
    inner
        .append(&[EventKind::AssistantMessage {
            turn,
            step,
            message,
            stream: attempt_stream.snapshot(),
            usage: usage.clone(),
            interrupted: None,
        }])
        .await?;
    emit_usage(inner, &usage);
    if finish == FinishReason::MaxTokens {
        // 被截断的输出里可能有不完整的工具调用：这一轮直接以 max-tokens 收尾，不执行工具。
        return Ok(StepEnd::MaxTokens);
    }
    let calls: Vec<ToolCall> = content
        .iter()
        .filter_map(ContentBlock::as_tool_call)
        .map(|(id, name, arguments)| ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        })
        .collect();
    if calls.is_empty() {
        return Ok(StepEnd::Completed);
    }
    // 先把 tool/call 作为持久事实写进日志，再派发（T004.4 的修复要靠它）。
    let mut call_events = Vec::with_capacity(calls.len());
    for call in &calls {
        call_events.push(EventKind::ToolCall {
            turn,
            step,
            call_id: call.id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
        });
    }
    inner.append(&call_events).await?;
    // T006.1d2：ToolBefore 挂点可返回 ToolResultOverride（ask 拦截）或 AbortStep（取消）。
    // override 的 index 对应调用跳过实际执行，结果按 calls 顺序并回。
    let dispatch = inner
        .capabilities
        .dispatch(&HookEvent::ToolBefore {
            turn,
            step,
            host: inner.as_ref(),
            calls: &calls,
        })
        .await?;
    if dispatch.abort {
        return Ok(StepEnd::Aborted);
    }
    let overrides = dispatch.overrides;
    let mut exec_calls: Vec<ToolCall> = Vec::new();
    let mut exec_indices: Vec<usize> = Vec::new();
    for (index, call) in calls.iter().enumerate() {
        if !overrides.iter().any(|(existing, _)| *existing == index) {
            exec_calls.push(call.clone());
            exec_indices.push(index);
        }
    }
    let context = ToolContext::with_session(inner.cwd.clone(), Arc::clone(&inner.tool_session));
    let mut outcome = execute_batch(
        &inner.tools,
        &exec_calls,
        inner.max_parallel_tool_calls,
        &context,
        abort.as_ref(),
    )
    .await;
    let mut merged: Vec<Option<tools::ToolResult>> = (0..calls.len()).map(|_| None).collect();
    for (position, result) in outcome.results.into_iter().enumerate() {
        if let Some(original) = exec_indices.get(position) {
            merged[*original] = Some(result);
        }
    }
    for (index, result) in overrides {
        if let Some(slot) = merged.get_mut(index) {
            *slot = Some(result);
        }
    }
    outcome.results = merged
        .into_iter()
        .map(|result| result.expect("每个调用都有结果（执行或 override）"))
        .collect();
    // T006.9c（照 DSH registry.start 的同步 append）：工具执行期间登记的后台任务先把
    // job/registered 落日志，再提交本批 tool/result；job/settled 仍由异步通道结算时写
    // （DSH jobs docs 允许晚于 turn/end）。
    let registered = inner.tool_session().tasks().take_registered_pending();
    if !registered.is_empty() {
        inner
            .append(&crate::jobs_capability::registered_events(&registered))
            .await?;
    }
    for result in &outcome.results {
        let Some(call) = calls.iter().find(|call| call.id == result.call_id) else {
            continue;
        };
        // T006.1d：落盘/内容变换由 ToolResult 挂点（spill 能力）决定；Continue 时用原文。
        let dispatch = inner
            .capabilities
            .dispatch(&HookEvent::ToolResult {
                turn,
                step,
                host: inner.as_ref(),
                call,
                result,
            })
            .await?;
        let content = dispatch.content.unwrap_or_else(|| result.content.clone());
        let message = Message::tool_result(result.call_id.clone(), content, result.is_error);
        inner
            .append(&[EventKind::ToolResult {
                turn,
                step,
                message,
                // 工具失败的身份照 DSH：name + code；普通参数错误没有结构化码，不写 error。
                error: result.error_code.as_ref().map(|code| ToolError {
                    name: "HarnessError".to_owned(),
                    code: code.clone(),
                }),
            }])
            .await?;
    }
    inner
        .capabilities
        .dispatch(&HookEvent::ToolAfter {
            turn,
            step,
            host: inner.as_ref(),
            calls: &calls,
            results: &outcome.results,
            cancelled: outcome.cancelled,
        })
        .await?;

    if abort.load(Ordering::SeqCst) || outcome.cancelled {
        return Ok(StepEnd::Aborted);
    }
    Ok(StepEnd::Continue)
}

/// 一条助手消息：source 是本次产出它的模型路由（DSH AssistantMessage 的 source=model）。
fn assistant_message(inner: &Arc<Inner>, content: Vec<ContentBlock>) -> Message {
    Message::assistant(
        content,
        inner.model.provider(),
        inner.model.config().model.clone(),
    )
}

/// 请求层重试：最多 5 次、500ms 起指数退避、上限 10s、±10% 抖动（std 派生，不加依赖）。
const MAX_RETRIES: u32 = 5;
const INITIAL_RETRY_DELAY_MS: u64 = 500;
const MAX_RETRY_DELAY_MS: u64 = 10_000;
const RETRY_JITTER_RATIO: f64 = 0.1;

/// spec 定的 5 个可重试码（排序后写进 policyKey，照 DSH retryPolicyKey）。
const RETRYABLE_CODES: [ErrorCode; 5] = [
    ErrorCode::EmptyResponse,
    ErrorCode::RateLimit,
    ErrorCode::Server,
    ErrorCode::Timeout,
    ErrorCode::Transport,
];

/// 固定重试策略的稳定标识（DSH llm-retry/src/index.ts:70-80 的 retryPolicyKey）。
fn retry_policy_key() -> String {
    let mut codes: Vec<String> = RETRYABLE_CODES.iter().map(|code| code.as_str()).collect();
    codes.sort();
    serde_json::json!([
        "normal",
        MAX_RETRIES,
        codes,
        INITIAL_RETRY_DELAY_MS,
        MAX_RETRY_DELAY_MS,
        RETRY_JITTER_RATIO,
    ])
    .to_string()
}

enum RetryDecision {
    Retry,
    Cancel,
    Fatal,
}

/// 只认 spec 定的 5 个可重试码。
fn is_retryable(code: ErrorCode) -> bool {
    RETRYABLE_CODES.contains(&code)
}

/// 这次失败要不要重试；要的话等完（可取消）再回 Retry。
///
/// T006.2b 照 DSH：已流出可见内容也重试（半截内容只留在 assistant/attempt 的 stream 里）；
/// 排定的重试先持久化 `llm/retry` 再做可取消等待，等待结束后、下一次尝试前落
/// `llm/retry-started`（DSH llm-retry/src/index.ts:166-186）。
async fn decide_retry(
    inner: &Arc<Inner>,
    cancel: &CancelToken,
    error: &ModelError,
    turn: u64,
    step: u64,
    attempt: &mut u32,
    retry_id: &mut Option<RetryId>,
) -> Result<RetryDecision, AgentError> {
    if !is_retryable(error.code) || *attempt >= MAX_RETRIES {
        return Ok(RetryDecision::Fatal);
    }
    let Some(delay_ms) = retry_delay_ms(error, *attempt + 1) else {
        return Ok(RetryDecision::Fatal);
    };
    *attempt += 1;
    // 同一条重试链共用一个 retryId（DSH 投影在 step/start、turn/end 后重置）。
    let retry_id = retry_id.get_or_insert_with(RetryId::generate).clone();
    // durable before the cancellable wait。
    inner
        .append(&[EventKind::LlmRetry {
            retry_id: retry_id.clone(),
            turn,
            step,
            provider: inner.model.provider().to_owned(),
            mode: RetryMode::Normal,
            policy_key: retry_policy_key(),
            retry: *attempt,
            max_retries: Some(MAX_RETRIES),
            delay_ms,
            failure: LlmFailure {
                message: error.message.clone(),
                code: error.code.as_str(),
            },
        }])
        .await?;
    let _ = inner.events.send(AgentEvent::Retrying {
        attempt: *attempt,
        max_attempts: MAX_RETRIES,
        delay_ms,
        code: error.code.as_str(),
    });
    let cancelled = tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(Duration::from_millis(delay_ms)) => false,
    };
    if cancelled {
        return Ok(RetryDecision::Cancel);
    }
    // 等待结束、下一次尝试开始前：llm/retry-started（DSH index.ts:184-186）。
    inner
        .append(&[EventKind::LlmRetryStarted {
            retry_id,
            turn,
            step,
            retry: *attempt,
        }])
        .await?;
    Ok(RetryDecision::Retry)
}

/// 服务端 retry-after 优先；超过上限就不重试（照 DSH normal 模式）；否则本地退避。
fn retry_delay_ms(error: &ModelError, retry: u32) -> Option<u64> {
    if let Some(after) = error.retry_after_ms {
        if after > MAX_RETRY_DELAY_MS {
            return None;
        }
        return Some(after.max(1));
    }
    let exponent = retry.saturating_sub(1).min(10);
    let exponential = INITIAL_RETRY_DELAY_MS
        .saturating_mul(1u64 << exponent)
        .min(MAX_RETRY_DELAY_MS);
    let factor = jitter_factor(retry);
    Some(((exponential as f64 * factor).round() as u64).clamp(1, MAX_RETRY_DELAY_MS))
}

/// std 派生伪随机抖动（SystemTime 纳秒 + 重试序号混合），不引 rand。
fn jitter_factor(retry: u32) -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos() as u64);
    let mixed = nanos ^ u64::from(retry).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let unit = (mixed >> 11) as f64 / (1u64 << 53) as f64;
    1.0 + 0.1 * (2.0 * unit - 1.0)
}

fn reset_attempt(
    blocks: &mut BTreeMap<usize, ContentBlock>,
    open: &mut BTreeMap<usize, (BlockType, String)>,
    usage: &mut Option<Usage>,
    finish: &mut Option<FinishReason>,
    stream: &mut AssistantStreamAccumulator,
) {
    blocks.clear();
    open.clear();
    *usage = None;
    *finish = None;
    *stream = AssistantStreamAccumulator::new();
}

fn emit_usage(inner: &Arc<Inner>, usage: &Option<Usage>) {
    if let Some(usage) = usage {
        let _ = inner.events.send(AgentEvent::UsageReported {
            usage: usage.clone(),
        });
    }
}

/// 取消时收尾一次已开始的尝试（DSH agent.ts:499-533）：有可见前缀写 interrupted 的
/// assistant/message；没有可见内容写 assistant/attempt（帧保留，不留空消息）。
async fn interrupted_step(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    blocks: &BTreeMap<usize, ContentBlock>,
    open: &BTreeMap<usize, (BlockType, String)>,
    usage: &Option<Usage>,
    stream: &AssistantStreamAccumulator,
) -> Result<StepEnd, AgentError> {
    let content = interrupted_content(blocks, open);
    if content.is_empty() {
        append_attempt(inner, turn, step, stream).await?;
    } else {
        let message = assistant_message(inner, content);
        inner
            .append(&[EventKind::AssistantMessage {
                turn,
                step,
                message,
                stream: stream.snapshot(),
                usage: usage.clone(),
                interrupted: Some(true),
            }])
            .await?;
        emit_usage(inner, usage);
    }
    Ok(StepEnd::Aborted)
}

/// 把一次没有产出可见消息的尝试落成 assistant/attempt（DSH agent.ts:567-580）。
async fn append_attempt(
    inner: &Arc<Inner>,
    turn: u64,
    step: u64,
    stream: &AssistantStreamAccumulator,
) -> Result<(), AgentError> {
    inner
        .append(&[EventKind::AssistantAttempt {
            turn,
            step,
            stream: stream.snapshot(),
        }])
        .await
}

/// 当前 Unix 毫秒时间（会话事件/流帧共用；系统时钟倒退时取 0）。
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// 取消时保留的可安全收尾前缀（照 DSH assembler.interruptedBlocks）：
/// 已关闭/打开中的 text 和 reasoning 块、去空白非空、按流顺序；tool-call 丢弃。
/// interrupted 的 thinking 不写 signature（照 DSH，handback 记）。
fn interrupted_content(
    blocks: &BTreeMap<usize, ContentBlock>,
    open: &BTreeMap<usize, (BlockType, String)>,
) -> Vec<ContentBlock> {
    let mut indexes: Vec<usize> = blocks.keys().chain(open.keys()).copied().collect();
    indexes.sort_unstable();
    indexes.dedup();
    let mut content = Vec::new();
    for index in indexes {
        let block = blocks.get(&index).cloned().or_else(|| {
            open.get(&index).map(|(block_type, text)| match block_type {
                BlockType::Text => ContentBlock::Text { text: text.clone() },
                BlockType::Reasoning => ContentBlock::Reasoning {
                    text: text.clone(),
                    signature: None,
                },
                BlockType::ToolCall => ContentBlock::Text { text: text.clone() },
            })
        });
        match block {
            Some(ContentBlock::Text { text }) if !text.trim().is_empty() => {
                content.push(ContentBlock::Text { text });
            }
            Some(ContentBlock::Reasoning { text, .. }) if !text.trim().is_empty() => {
                content.push(ContentBlock::Reasoning {
                    text,
                    signature: None,
                });
            }
            _ => {}
        }
    }
    content
}

/// 系统提示：固定文字 + 会话的工作目录；按可见工具补指导句（照 DSH 的 tool-fs 各工具
/// 系统提示段，插件名换成 AIOS 的规则表述）。
fn system_prompt(cwd: &Path, tools: &ToolSet) -> String {
    let mut prompt = format!(
        "You are AIOS, a coding agent. Your working directory is {}.",
        cwd.display()
    );
    if tools.get("read").is_some() {
        prompt.push_str(
            "\n\nUse the read tool — not shell commands like cat — to inspect text files. Use offset and limit to continue reading large files.",
        );
    }
    if tools.get("bash").is_some() {
        prompt.push_str(
            "\n\nCheck the [exit code: N] marker on every bash result; investigate failures before moving on.",
        );
    }
    if tools.get("job_output").is_some() {
        prompt.push_str(
            "\n\nTrack every background job id you start. You are notified in-session when a job finishes — do not busy-poll or sleep on one; keep working on independent steps and do not duplicate a running job's work. Before giving a final answer, collect every still-relevant job with job_output (set wait: true only when you are genuinely blocked on it), and job_kill jobs that stopped mattering.",
        );
    }
    if tools.get("write").is_some() {
        prompt.push_str("\n\nRead an existing file before overwriting it with write");
        if tools.get("edit").is_some() {
            prompt.push_str("; prefer edit for targeted changes");
        }
        prompt.push('.');
    }
    if tools.get("edit").is_some() {
        prompt.push_str(
            "\n\nRead a file before editing it, unless you just created or edited it in this session.",
        );
    }
    if tools.get("apply_patch").is_some() {
        prompt.push_str(
            "\n\nPrefer apply_patch over separate edit calls for changes across files or separated regions. Read a file before updating, deleting, or moving it, unless you just created or edited it in this session. The whole patch is checked first and a rejected patch changes nothing; once applying starts, operations run in order, stop at the first failure, and are not rolled back, so re-read affected files before retrying when needed.",
        );
    }
    prompt
}

#[cfg(test)]
mod settle_tests {
    use super::*;

    /// settle 边界确定性测试：用 test-only hook 把 settle 卡在「预检完成、
    /// 最终临界区之前」，窗口内入队 compact 并模拟 wake_driver 只看到 Running，
    /// 然后断言 Agent::compact 的 oneshot 在超时内收到通知（不看 phase、不 sleep）。
    #[tokio::test]
    async fn settle_requeues_waiter_arriving_at_boundary() {
        let data = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = SessionStore::create(data.path(), cwd.path()).unwrap();
        let model = model::DeepSeek::new(model::DeepSeekConfig {
            api_key: "test-key".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            ..model::DeepSeekConfig::default()
        })
        .unwrap();
        let inner = Inner::start(
            crate::AgentConfig::default(),
            Deps {
                model,
                tools: ToolSet::empty(),
                store,
            },
            0,
        )
        .unwrap();
        {
            let mut state = inner.state.lock().unwrap();
            state.phase = Phase::Running {
                abort: Arc::new(AtomicBool::new(false)),
                turn: 0,
            };
        }
        let (entered_tx, entered_rx) = std::sync::mpsc::channel::<()>();
        let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
        let release_rx = std::sync::Mutex::new(release_rx);
        let release_rx = std::sync::Arc::new(release_rx);
        let fired = Arc::new(std::sync::atomic::AtomicBool::new(false));
        {
            let mut hook = inner.settle_hook.lock().unwrap();
            *hook = Some(Arc::new(move || {
                // 只卡第一次 settle（被测窗口）；后续 kick 里的 settle 直接放行，
                // 否则会在 async worker 上阻塞标准库 recv、饿死运行时。
                if fired.swap(true, std::sync::atomic::Ordering::SeqCst) {
                    return;
                }
                let _ = entered_tx.send(());
                let _ = release_rx.lock().unwrap().recv();
            }));
        }
        let arc = Arc::clone(&inner);
        let settle = tokio::task::spawn_blocking(move || settle_idle(&arc));
        // 等 settle 停在临界区之前（确定性，不靠 sleep）。
        entered_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        // 窗口内：入队 waiter，并模拟 wake_driver 只看到 Running（只置 latch）。
        let (reply, receiver) = tokio::sync::oneshot::channel();
        let request = crate::capabilities::ManualCompaction {
            source_command_id: None,
            reply,
        };
        let _ = inner.capabilities.push_compaction_waiter(request);
        inner.wake_driver();
        let _ = release_tx.send(());
        settle.await.unwrap();
        let served = tokio::time::timeout(std::time::Duration::from_secs(5), receiver)
            .await
            .is_ok();
        // 清理：避免随后 kick 真的尝试压缩。
        let _ = inner.capabilities.take_compaction_waiters();
        inner.state.lock().unwrap().phase = Phase::Idle;
        assert!(served, "边界入队的 compact waiter 必须在超时内收到通知");
    }
}
