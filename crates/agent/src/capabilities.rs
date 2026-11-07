//! loop 的固定挂点、功能注册表与 Capability 输入面（T006.1a 骨架，行为不变）。
//!
//! 挂点照 strategy §1；Rust 接法是编译期固定 trait 列表 + 固定顺序注册表（无运行时加载）。
//! 1a 的 Capability 全是 no-op 占位，但输入面按 1b–1e 的需要定：异步、可返回错误、
//! 每个挂点带 typed HookEvent（host 访问面 + 该挂点的运行态数据）。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use model::{DeepSeek, Message, ModelError};
use session::{EventKind, SessionStore};
use tokio::sync::broadcast;
use tools::{ToolCall, ToolResult, ToolSessionState, ToolSet};

use crate::ask_capability::{AskCapability, AskState};
use crate::compaction_capability::CompactionCapability;
use crate::error::AgentError;
use crate::instructions::InstructionState;
use crate::instructions_capability::InstructionsCapability;
use crate::prune_capability::PruneCapability;
use crate::reminder_capability::ReminderCapability;
use crate::skill_capability::SkillCapability;
use crate::skill_catalog::SkillCatalogState;
use crate::spill_capability::SpillCapability;
use crate::todo_capability::TodoCapability;
use crate::{AgentEvent, CompactionTrigger};

/// PreStep 的两阶段：Inject = 注入指令/skill（在 prune 之前）；Compact = 压缩自动检查（在 prune 之后）。
/// 拆分是为了保持 T005 的旧顺序：注入 → prune → 压缩，同时不新增挂点。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PreStepPhase {
    Inject,
    Compact,
}

/// loop 的固定挂点（strategy §1 六点位；工具拆三阶段；压缩触发单列）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HookPoint {
    /// 每步之前：压缩检查、指令注入、skill 目录。
    PreStep,
    /// 发请求时：压缩 token 记账。
    Request,
    /// 请求报错：溢出后压缩重试。
    RequestError,
    /// 写日志事件后：指令对账（对外推送后续接）。
    SessionEvent,
    /// 工具执行前：文件观察、指令发现。
    ToolBefore,
    /// 工具出结果：大结果落盘。
    ToolResult,
    /// 工具执行后：落盘收尾/后处理。
    ToolAfter,
    /// 压缩触发：剪枝只在压缩触发时发生（T006.4）。
    Compaction,
    /// 状态变化：对外推送（服务接入后使用）。
    Status,
    /// 本轮要停：对外推送收尾（服务接入后使用）。
    TurnStopping,
}

/// 解析后的压缩策略（DSH ResolvedTargetPolicy 的 AIOS 子集；T006.3）。
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CompactionPolicy {
    pub threshold_ratio: f64,
    pub retain_ratio: f64,
    pub headroom_tokens: u64,
    pub summary_max_tokens: u32,
    pub compaction_retries: u32,
    pub max_overflow_retries: u32,
}

/// loop 对 Capability 暴露的访问面；由 driver 的 Inner 实现。
/// 访问面按宿主完整形状定义，个别方法当前还没有能力读取。
#[allow(dead_code)]
/// 后续功能搬出去时只依赖这个 trait，不直接依赖 Inner 的私有字段。
pub(crate) trait LoopAccess: Send + Sync {
    fn session(&self) -> &Mutex<SessionStore>;
    fn events(&self) -> &broadcast::Sender<AgentEvent>;
    fn tools(&self) -> &ToolSet;
    fn tool_session(&self) -> &Arc<ToolSessionState>;
    fn model(&self) -> &DeepSeek;
    fn cwd(&self) -> &Path;
    fn data_dir(&self) -> &Path;
    fn log_path(&self) -> &Path;
    fn compaction_policy(&self) -> CompactionPolicy;
    /// 写日志入口：内部走 `Inner::append`（落盘后逐事件派发 SessionEvent）；禁止直接锁
    /// `session()` 后 append 绕过 SessionEvent。
    fn append<'a>(&'a self, kinds: &'a [EventKind]) -> HookFuture<'a>;
    /// 追加一批事件并返回分配的 seq（T006.3：压缩要拿 start/summary 的 seq 拼 sourceEventSeqs）。
    fn append_with_seqs<'a>(&'a self, kinds: &'a [EventKind]) -> BoxedFuture<'a, Vec<u64>>;
    /// 追加一条 message-producing 事件并带上 envelope 的 surfaceOp/sourceEventSeqs（压缩替换）。
    fn append_surface<'a>(
        &'a self,
        kind: EventKind,
        surface_op: session::SurfaceOp,
        source_event_seqs: Vec<u64>,
    ) -> HookFuture<'a>;
    /// 压缩确认后统一派发 `HookEvent::Compaction`（供 prune 等；自动/溢出/手动三路共用）。
    fn dispatch_compaction<'a>(
        &'a self,
        turn: u64,
        step: u64,
        trigger: CompactionTrigger,
    ) -> HookFuture<'a>;
    /// 当前轮次的 abort 标志（ask 能力等待答案时要检查取消；Running 时总有）。
    fn current_abort(&self) -> Option<Arc<AtomicBool>>;
    /// 把一段上下文消息投进会话（后台任务结算通知用；按 phase 选 NextStep/NextTurn）。
    /// T006.2a 起走同一条 durable splice（返回 future，调用方 await 落盘后再 wake）。
    fn enqueue_context<'a>(&'a self, text: String) -> BoxedFuture<'a, ()>;
    /// T006.F1：投递一条已构造好的消息（提问迟到回答用）；落盘后负责唤醒驱动。
    fn enqueue_message<'a>(&'a self, message: Message) -> BoxedFuture<'a, bool>;
    /// 当前是否有一轮在跑（手动压缩 busy 判定）。
    fn is_running(&self) -> bool;
    /// 最近一轮的 turn 号（手动压缩写进 compaction/* 事件）。
    fn last_turn(&self) -> u64;
}

/// 每个挂点一份 typed 事件：host 访问面 + 该挂点可用的运行态数据。
/// 字段按挂点完整形状定义，个别字段当前还没有能力读取。
#[allow(dead_code)]
pub(crate) enum HookEvent<'a> {
    PreStep {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        /// 这一步 claim 到的 user 消息（T006.11b：skill 的 /name 只扫 source=user 的消息）。
        messages: &'a [Message],
        /// None = 每步自动检查；Some(Manual) = 手动 /compact 入口（强制压缩）。
        trigger: Option<CompactionTrigger>,
        /// 1c：PreStep 拆两阶段（Inject 在 prune 前；Compact 在 prune 后）。
        phase: PreStepPhase,
    },
    Request {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        messages: &'a [Message],
    },
    RequestError {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        error: &'a ModelError,
    },
    SessionEvent {
        host: &'a dyn LoopAccess,
        event: &'a EventKind,
    },
    ToolBefore {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        calls: &'a [ToolCall],
    },
    ToolResult {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        call: &'a ToolCall,
        result: &'a ToolResult,
    },
    ToolAfter {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        calls: &'a [ToolCall],
        results: &'a [ToolResult],
        cancelled: bool,
    },
    Compaction {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        trigger: CompactionTrigger,
    },
    Status {
        turn: u64,
        host: &'a dyn LoopAccess,
    },
    TurnStopping {
        turn: u64,
        host: &'a dyn LoopAccess,
    },
}

impl HookEvent<'_> {
    pub(crate) fn hook(&self) -> HookPoint {
        match self {
            Self::PreStep { .. } => HookPoint::PreStep,
            Self::Request { .. } => HookPoint::Request,
            Self::RequestError { .. } => HookPoint::RequestError,
            Self::SessionEvent { .. } => HookPoint::SessionEvent,
            Self::ToolBefore { .. } => HookPoint::ToolBefore,
            Self::ToolResult { .. } => HookPoint::ToolResult,
            Self::ToolAfter { .. } => HookPoint::ToolAfter,
            Self::Compaction { .. } => HookPoint::Compaction,
            Self::Status { .. } => HookPoint::Status,
            Self::TurnStopping { .. } => HookPoint::TurnStopping,
        }
    }

    pub(crate) fn host(&self) -> &dyn LoopAccess {
        match self {
            Self::PreStep { host, .. }
            | Self::Request { host, .. }
            | Self::RequestError { host, .. }
            | Self::SessionEvent { host, .. }
            | Self::ToolBefore { host, .. }
            | Self::ToolResult { host, .. }
            | Self::ToolAfter { host, .. }
            | Self::Compaction { host, .. }
            | Self::Status { host, .. }
            | Self::TurnStopping { host, .. } => *host,
        }
    }

    pub(crate) fn turn(&self) -> u64 {
        match self {
            Self::PreStep { turn, .. }
            | Self::Request { turn, .. }
            | Self::RequestError { turn, .. }
            | Self::ToolBefore { turn, .. }
            | Self::ToolResult { turn, .. }
            | Self::ToolAfter { turn, .. }
            | Self::Compaction { turn, .. }
            | Self::Status { turn, .. }
            | Self::TurnStopping { turn, .. } => *turn,
            Self::SessionEvent { event, .. } => event_context(event).map_or(0, |(turn, _)| turn),
        }
    }

    pub(crate) fn step(&self) -> u64 {
        match self {
            Self::PreStep { step, .. }
            | Self::Request { step, .. }
            | Self::RequestError { step, .. }
            | Self::ToolBefore { step, .. }
            | Self::ToolResult { step, .. }
            | Self::ToolAfter { step, .. }
            | Self::Compaction { step, .. } => *step,
            Self::Status { .. } | Self::TurnStopping { .. } => 0,
            Self::SessionEvent { event, .. } => event_context(event).map_or(0, |(_, step)| step),
        }
    }
}

/// 挂点返回值（四变体，T006.1d2）：Content 只允许 ToolResult 阶段，Override 只允许
/// ToolBefore 阶段；AbortStep 表示本 step 应 aborted 收尾（不是 error）。
#[derive(Clone)]
pub(crate) enum HookOutcome {
    Continue,
    /// ToolResult 挂点用：用落盘后的 preview+locator 替换内联 content。
    ToolResultContent(String),
    /// ToolBefore 挂点用：这些 index 的调用不执行，直接用给定结果（批内顺序不变；
    /// T006.13b 起批量返回，批内多个 ask 依次拦截）。
    ToolResultOverrides {
        overrides: Vec<(usize, ToolResult)>,
    },
    /// 本 step 取消（ask 等待中被 cancel）；run_step 返回 StepEnd::Aborted。
    AbortStep,
}

/// dyn 兼容的异步返回；不加 async-trait 依赖。
pub(crate) type HookFuture<'a> =
    Pin<Box<dyn Future<Output = Result<HookOutcome, AgentError>> + Send + 'a>>;

/// 非挂点异步入口的返回（输入队列写入等要 await 落盘 + 钩子）。
pub(crate) type BoxedFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentError>> + Send + 'a>>;

/// dispatch 聚合结果：overrides（同 index 后写胜出）、最后非 None 的 content、abort 优先。
#[derive(Default)]
pub(crate) struct DispatchOutcome {
    pub overrides: Vec<(usize, ToolResult)>,
    pub content: Option<String>,
    pub abort: bool,
}

/// 一个可挂到 loop 挂点上的功能（编译期 trait，不做运行时加载）。
pub(crate) trait Capability: Send + Sync {
    fn name(&self) -> &'static str;
    fn hooks(&self) -> &'static [HookPoint];
    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a>;
    // 以下为 loop 少量非挂点入口（默认 no-op，由对应能力实现；T006.1e 把状态收回能力）。
    /// 回答一条待答问题（T006.F1：迟到回答的 durable 编排也在能力里；driver 只派发）。
    fn answer_with<'a>(
        &'a self,
        _host: &'a dyn LoopAccess,
        _question_id: &'a str,
        _selected: Vec<String>,
        _custom: Option<String>,
    ) -> BoxedFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }
    /// 撤销一条待答问题（T006.F1：恢复批的事件编排也在能力里；driver 只派发）。
    fn cancel_question_with<'a>(
        &'a self,
        _host: &'a dyn LoopAccess,
        _question_id: &'a str,
    ) -> BoxedFuture<'a, bool> {
        Box::pin(async { Ok(false) })
    }
    /// T006.F1：手动压缩入口（照 command-compact busy；不排队）。None = 本能力不处理。
    fn compact_manual_with<'a>(
        &'a self,
        _host: &'a dyn LoopAccess,
        _source_command_id: Option<String>,
    ) -> BoxedFuture<'a, Option<Result<Option<crate::CompactionOutcome>, AgentError>>> {
        Box::pin(async { Ok(None) })
    }
    /// 当前待答问题（T006.13，含重启恢复；UI 服务 T006.16a 用）。
    fn pending_questions(&self) -> Vec<crate::Question> {
        Vec::new()
    }
    fn cancel_questions(&self) {}
    fn reset_repeat_chain(&self) {}
}

/// 占位登记：只声明功能与它关心的挂点，不接管行为。
struct Placeholder {
    name: &'static str,
    hooks: &'static [HookPoint],
}

impl Capability for Placeholder {
    fn name(&self) -> &'static str {
        self.name
    }
    fn hooks(&self) -> &'static [HookPoint] {
        self.hooks
    }
    fn on_hook<'a>(&'a self, _event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async { Ok(HookOutcome::Continue) })
    }
}

const JOBS_HOOKS: &[HookPoint] = &[HookPoint::Status];

/// per-Agent 能力状态包（T006.1e：状态由能力模块持有，driver 只装配一次）。
pub(crate) struct CapabilityStates {
    pub ask: Arc<AskState>,
    /// T006.10：prepare 异步读指令文件，状态用 tokio Mutex 以便持锁跨 await。
    pub instructions: Arc<tokio::sync::Mutex<InstructionState>>,
    pub skill_catalog: Arc<Mutex<SkillCatalogState>>,
    pub reminder_chain: Arc<Mutex<Option<(String, usize)>>>,
    pub instruction_roots: Vec<PathBuf>,
    pub skill_roots: Vec<PathBuf>,
    pub spill_max_inline_tokens: usize,
}
/// 固定顺序的功能注册表；顺序=strategy §2 功能表的拆票顺序（compaction→…→jobs）。
/// per-Agent 构造（T006.1d2）：ZST 能力每 Agent Box 一份，ask 能力持有该 Agent 的 AskState。
pub(crate) struct CapabilityRegistry {
    capabilities: Vec<Box<dyn Capability>>,
}

impl CapabilityRegistry {
    pub(crate) fn production(states: CapabilityStates) -> Self {
        Self {
            capabilities: vec![
                Box::new(CompactionCapability::new()),
                Box::new(PruneCapability),
                Box::new(InstructionsCapability::new((
                    states.instructions,
                    states.instruction_roots,
                ))),
                Box::new(SkillCapability::new((
                    states.skill_catalog,
                    states.skill_roots,
                ))),
                Box::new(TodoCapability),
                Box::new(AskCapability::new(states.ask)),
                Box::new(SpillCapability::new(states.spill_max_inline_tokens)),
                Box::new(ReminderCapability::new(states.reminder_chain)),
                Box::new(Placeholder {
                    name: "jobs",
                    hooks: JOBS_HOOKS,
                }),
            ],
        }
    }

    pub(crate) async fn answer(
        &self,
        host: &dyn LoopAccess,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> bool {
        for capability in &self.capabilities {
            if matches!(
                capability
                    .answer_with(host, question_id, selected.clone(), custom.clone())
                    .await,
                Ok(true)
            ) {
                return true;
            }
        }
        false
    }

    pub(crate) async fn cancel_question(&self, host: &dyn LoopAccess, question_id: &str) -> bool {
        for capability in &self.capabilities {
            if matches!(
                capability.cancel_question_with(host, question_id).await,
                Ok(true)
            ) {
                return true;
            }
        }
        false
    }

    pub(crate) async fn compact_manual(
        &self,
        host: &dyn LoopAccess,
        source_command_id: Option<String>,
    ) -> Result<Option<crate::CompactionOutcome>, AgentError> {
        for capability in &self.capabilities {
            if let Some(result) = capability
                .compact_manual_with(host, source_command_id.clone())
                .await?
            {
                return result;
            }
        }
        Err(AgentError::ManualCompaction {
            code: crate::ManualCompactionErrorCode::Commit,
            message: "压缩能力未注册".to_owned(),
        })
    }

    pub(crate) fn pending_questions(&self) -> Vec<crate::Question> {
        let mut questions = Vec::new();
        for capability in &self.capabilities {
            questions.extend(capability.pending_questions());
        }
        questions
    }

    pub(crate) fn cancel_questions(&self) {
        for capability in &self.capabilities {
            capability.cancel_questions();
        }
    }

    pub(crate) fn reset_repeat_chain(&self) {
        for capability in &self.capabilities {
            capability.reset_repeat_chain();
        }
    }

    #[cfg(test)]
    fn from_capabilities(capabilities: Vec<Box<dyn Capability>>) -> Self {
        Self { capabilities }
    }

    /// 按注册顺序派发；首个 Err 短路（之前的功能已执行完，不回滚）。
    /// 阶段不匹配的 outcome 被忽略（Content 只 ToolResult、Override 只 ToolBefore）。
    pub(crate) async fn dispatch(
        &self,
        event: &HookEvent<'_>,
    ) -> Result<DispatchOutcome, AgentError> {
        let mut outcome = DispatchOutcome::default();
        for capability in &self.capabilities {
            debug_assert!(!capability.name().is_empty());
            if capability.hooks().contains(&event.hook()) {
                let next = capability.on_hook(event).await?;
                let is_abort = matches!(next, HookOutcome::AbortStep);
                Self::merge(&mut outcome, event.hook(), next);
                // AbortStep 优先且立即停止：后续能力不再执行（不会 append/override/Err）。
                if is_abort {
                    break;
                }
            }
        }
        Ok(outcome)
    }

    fn merge(outcome: &mut DispatchOutcome, hook: HookPoint, next: HookOutcome) {
        match next {
            HookOutcome::Continue => {}
            HookOutcome::ToolResultContent(content) => {
                if hook == HookPoint::ToolResult {
                    outcome.content = Some(content);
                }
            }
            HookOutcome::ToolResultOverrides { overrides } => {
                if hook == HookPoint::ToolBefore {
                    for (index, result) in overrides {
                        outcome.overrides.retain(|(existing, _)| *existing != index);
                        outcome.overrides.push((index, result));
                    }
                }
            }
            HookOutcome::AbortStep => outcome.abort = true,
        }
    }
}
/// 从一条日志事件取（turn, step）；没有 turn/step 的事件返回 None。
pub(crate) fn event_context(kind: &EventKind) -> Option<(u64, u64)> {
    let pair = match kind {
        EventKind::TurnStart { turn } | EventKind::TurnEnd { turn, .. } => (*turn, 0),
        EventKind::StepStart { turn, step }
        | EventKind::StepEnd { turn, step }
        | EventKind::SystemMessage { turn, step, .. }
        | EventKind::UserMessage { turn, step, .. }
        | EventKind::AssistantMessage { turn, step, .. }
        | EventKind::AssistantAttempt { turn, step, .. }
        | EventKind::LlmRetry { turn, step, .. }
        | EventKind::LlmRetryStarted { turn, step, .. }
        | EventKind::ToolCall { turn, step, .. }
        | EventKind::ToolResult { turn, step, .. }
        | EventKind::QuestionAsked { turn, step, .. }
        | EventKind::QuestionAnswered { turn, step, .. }
        | EventKind::QuestionCancelled { turn, step, .. } => (*turn, *step),
        EventKind::CompactionStart { turn, .. } | EventKind::CompactionEnd { turn, .. } => {
            // 手动压缩在两次 turn 之间进行（DSH turn: null），没有 turn 上下文。
            ((*turn)?, 0)
        }
        EventKind::TodoWrite { .. }
        | EventKind::InboxSpliced { .. }
        | EventKind::JobRegistered { .. }
        | EventKind::JobSettled { .. }
        | EventKind::CompactionSummary { .. }
        | EventKind::CompactionPrune { .. }
        | EventKind::SessionModel { .. } => return None,
    };
    Some(pair)
}

#[cfg(test)]
mod dispatch_contract_tests {
    use super::*;
    use std::path::Path;
    use tools::{ToolCall, ToolSet};

    struct NoopHost;

    impl LoopAccess for NoopHost {
        fn session(&self) -> &Mutex<SessionStore> {
            unimplemented!()
        }
        fn events(&self) -> &broadcast::Sender<AgentEvent> {
            unimplemented!()
        }
        fn tools(&self) -> &ToolSet {
            unimplemented!()
        }
        fn tool_session(&self) -> &Arc<ToolSessionState> {
            unimplemented!()
        }
        fn model(&self) -> &DeepSeek {
            unimplemented!()
        }
        fn cwd(&self) -> &Path {
            unimplemented!()
        }
        fn data_dir(&self) -> &Path {
            unimplemented!()
        }
        fn log_path(&self) -> &Path {
            unimplemented!()
        }
        fn compaction_policy(&self) -> CompactionPolicy {
            unimplemented!()
        }
        fn append<'a>(&'a self, _kinds: &'a [EventKind]) -> HookFuture<'a> {
            unimplemented!()
        }
        fn append_with_seqs<'a>(&'a self, _kinds: &'a [EventKind]) -> BoxedFuture<'a, Vec<u64>> {
            unimplemented!()
        }
        fn append_surface<'a>(
            &'a self,
            _kind: EventKind,
            _surface_op: session::SurfaceOp,
            _source_event_seqs: Vec<u64>,
        ) -> HookFuture<'a> {
            unimplemented!()
        }
        fn dispatch_compaction<'a>(
            &'a self,
            _turn: u64,
            _step: u64,
            _trigger: crate::CompactionTrigger,
        ) -> HookFuture<'a> {
            unimplemented!()
        }
        fn current_abort(&self) -> Option<Arc<AtomicBool>> {
            unimplemented!()
        }
        fn enqueue_context<'a>(&'a self, _text: String) -> BoxedFuture<'a, ()> {
            unimplemented!()
        }
        fn enqueue_message<'a>(&'a self, _message: Message) -> BoxedFuture<'a, bool> {
            unimplemented!()
        }
        fn is_running(&self) -> bool {
            false
        }
        fn last_turn(&self) -> u64 {
            0
        }
    }

    struct Fixed {
        name: &'static str,
        hooks: &'static [HookPoint],
        outcome: HookOutcome,
    }

    impl Capability for Fixed {
        fn name(&self) -> &'static str {
            self.name
        }
        fn hooks(&self) -> &'static [HookPoint] {
            self.hooks
        }
        fn on_hook<'a>(&'a self, _event: &'a HookEvent<'a>) -> HookFuture<'a> {
            let outcome = self.outcome.clone();
            Box::pin(async move { Ok(outcome) })
        }
    }

    fn fixed(
        name: &'static str,
        hooks: &'static [HookPoint],
        outcome: HookOutcome,
    ) -> Box<dyn Capability> {
        Box::new(Fixed {
            name,
            hooks,
            outcome,
        })
    }

    fn tool_before_event<'a>(host: &'a NoopHost, calls: &'a [ToolCall]) -> HookEvent<'a> {
        HookEvent::ToolBefore {
            turn: 1,
            step: 1,
            host,
            calls,
        }
    }

    fn tool_result_event<'a>(
        host: &'a NoopHost,
        call: &'a ToolCall,
        result: &'a ToolResult,
    ) -> HookEvent<'a> {
        HookEvent::ToolResult {
            turn: 1,
            step: 1,
            host,
            call,
            result,
        }
    }

    #[tokio::test]
    async fn overrides_are_collected_by_registration_order_and_last_write_wins() {
        let registry = CapabilityRegistry::from_capabilities(vec![
            fixed(
                "test-a",
                &[HookPoint::ToolBefore],
                HookOutcome::ToolResultOverrides {
                    overrides: vec![(0, ToolResult::success("call-0", "A"))],
                },
            ),
            fixed(
                "test-b",
                &[HookPoint::ToolBefore],
                HookOutcome::ToolResultOverrides {
                    overrides: vec![(0, ToolResult::success("call-0", "B"))],
                },
            ),
            fixed(
                "test-c",
                &[HookPoint::ToolBefore],
                HookOutcome::ToolResultOverrides {
                    overrides: vec![(1, ToolResult::success("call-1", "C"))],
                },
            ),
        ]);
        let host = NoopHost;
        let outcome = registry
            .dispatch(&tool_before_event(&host, &[]))
            .await
            .unwrap();
        assert!(!outcome.abort);
        assert_eq!(outcome.overrides.len(), 2);
        assert_eq!(outcome.overrides[0].0, 0);
        assert_eq!(outcome.overrides[0].1.content, "B", "同 index 后写胜出");
        assert_eq!(outcome.overrides[1].0, 1);
        assert_eq!(outcome.overrides[1].1.content, "C");
    }

    struct Recorder {
        calls: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl Capability for Recorder {
        fn name(&self) -> &'static str {
            "test-recorder"
        }
        fn hooks(&self) -> &'static [HookPoint] {
            &[HookPoint::ToolBefore]
        }
        fn on_hook<'a>(&'a self, _event: &'a HookEvent<'a>) -> HookFuture<'a> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async { Ok(HookOutcome::Continue) })
        }
    }

    #[tokio::test]
    async fn abort_stops_dispatch_immediately_without_error() {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let registry = CapabilityRegistry::from_capabilities(vec![
            fixed(
                "test-abort",
                &[HookPoint::ToolBefore],
                HookOutcome::AbortStep,
            ),
            Box::new(Recorder {
                calls: Arc::clone(&calls),
            }),
        ]);
        let host = NoopHost;
        let outcome = registry
            .dispatch(&tool_before_event(&host, &[]))
            .await
            .expect("AbortStep 不应返回 Err");
        assert!(outcome.abort);
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            0,
            "abort 之后的能力不得被调用"
        );
    }

    #[tokio::test]
    async fn abort_takes_priority_over_overrides() {
        let registry = CapabilityRegistry::from_capabilities(vec![
            fixed(
                "test-override",
                &[HookPoint::ToolBefore],
                HookOutcome::ToolResultOverrides {
                    overrides: vec![(0, ToolResult::success("call-0", "X"))],
                },
            ),
            fixed(
                "test-abort",
                &[HookPoint::ToolBefore],
                HookOutcome::AbortStep,
            ),
        ]);
        let host = NoopHost;
        let outcome = registry
            .dispatch(&tool_before_event(&host, &[]))
            .await
            .unwrap();
        assert!(outcome.abort, "AbortStep 优先");
        assert_eq!(outcome.overrides.len(), 1, "override 仍被收集");
    }

    #[tokio::test]
    async fn content_is_last_non_none_and_only_on_tool_result() {
        let registry = CapabilityRegistry::from_capabilities(vec![
            fixed(
                "test-content-a",
                &[HookPoint::ToolResult],
                HookOutcome::ToolResultContent("A".to_owned()),
            ),
            fixed(
                "test-content-b",
                &[HookPoint::ToolResult],
                HookOutcome::ToolResultContent("B".to_owned()),
            ),
        ]);
        let host = NoopHost;
        let call = ToolCall {
            id: "call-0".to_owned(),
            name: "read".to_owned(),
            arguments: "{}".to_owned(),
        };
        let result = ToolResult::success("call-0", "raw");
        let outcome = registry
            .dispatch(&tool_result_event(&host, &call, &result))
            .await
            .unwrap();
        assert_eq!(outcome.content.as_deref(), Some("B"));

        // 阶段不匹配：Override 在 ToolResult 阶段被忽略。
        let registry = CapabilityRegistry::from_capabilities(vec![fixed(
            "test-wrong-stage",
            &[HookPoint::ToolResult],
            HookOutcome::ToolResultOverrides {
                overrides: vec![(0, ToolResult::success("call-0", "bad"))],
            },
        )]);
        let outcome = registry
            .dispatch(&tool_result_event(&host, &call, &result))
            .await
            .unwrap();
        assert!(outcome.overrides.is_empty());
        assert!(outcome.content.is_none());
    }
}
