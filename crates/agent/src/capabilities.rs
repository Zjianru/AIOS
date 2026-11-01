//! loop 的固定挂点、功能注册表与 Capability 输入面（T006.1a 骨架，行为不变）。
//!
//! 挂点照 strategy §1；Rust 接法是编译期固定 trait 列表 + 固定顺序注册表（无运行时加载）。
//! 1a 的 Capability 全是 no-op 占位，但输入面按 1b–1e 的需要定：异步、可返回错误、
//! 每个挂点带 typed HookEvent（host 访问面 + 该挂点的运行态数据）。

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use model::{DeepSeek, Message, ModelError};
use session::{EventKind, SessionStore};
use tokio::sync::broadcast;
use tools::{ToolCall, ToolResult, ToolSessionState, ToolSet};

use crate::compaction_capability::CompactionCapability;
use crate::error::AgentError;
use crate::instructions::InstructionState;
use crate::instructions_capability::InstructionsCapability;
use crate::skill_capability::SkillCapability;
use crate::skill_catalog::SkillCatalogState;
use crate::{AgentEvent, CompactionTrigger};

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

/// 压缩相关配置的只读快照（1b 的压缩实现从这里取值）。
#[allow(dead_code)] // 1a 先定输入面，1b 起由功能实现读取
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CompactionConfig {
    pub threshold_ratio: f64,
    pub retain_ratio: f64,
    pub headroom_tokens: u64,
}

/// loop 对 Capability 暴露的访问面；由 driver 的 Inner 实现。
/// 后续功能搬出去时只依赖这个 trait，不直接依赖 Inner 的私有字段。
#[allow(dead_code)] // 1a 先定输入面，1b–1e 由功能实现读取
pub(crate) trait LoopAccess: Send + Sync {
    fn session(&self) -> &Mutex<SessionStore>;
    fn events(&self) -> &broadcast::Sender<AgentEvent>;
    fn tools(&self) -> &ToolSet;
    fn tool_session(&self) -> &Arc<ToolSessionState>;
    fn model(&self) -> &DeepSeek;
    fn cwd(&self) -> &Path;
    fn data_dir(&self) -> &Path;
    fn log_path(&self) -> &Path;
    fn instruction_roots(&self) -> &[PathBuf];
    fn skill_roots(&self) -> &[PathBuf];
    fn compaction_config(&self) -> CompactionConfig;
    fn spill_max_inline_tokens(&self) -> usize;
    /// 写日志入口：内部走 `Inner::append`（落盘后逐事件派发 SessionEvent）；禁止直接锁
    /// `session()` 后 append 绕过 SessionEvent。
    fn append<'a>(&'a self, kinds: &'a [EventKind]) -> HookFuture<'a>;
    /// 压缩确认后统一派发 `HookEvent::Compaction`（供 prune 等；自动/溢出/手动三路共用）。
    fn dispatch_compaction<'a>(
        &'a self,
        turn: u64,
        step: u64,
        trigger: CompactionTrigger,
    ) -> HookFuture<'a>;
    /// 指令注入状态（T005.5；指令能力在 PreStep 里读写）。
    fn instructions_state(&self) -> &Mutex<InstructionState>;
    /// skill 目录注入状态（T005.6；skill 能力在 PreStep 里读写）。
    fn skill_catalog_state(&self) -> &Mutex<SkillCatalogState>;
}

/// 每个挂点一份 typed 事件：host 访问面 + 该挂点可用的运行态数据。
#[allow(dead_code)] // 1a 只定义每个挂点的 typed 数据面，1b–1e 读取
pub(crate) enum HookEvent<'a> {
    PreStep {
        turn: u64,
        step: u64,
        host: &'a dyn LoopAccess,
        /// None = 每步自动检查；Some(Manual) = 手动 /compact 入口（强制压缩）。
        trigger: Option<CompactionTrigger>,
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

#[allow(dead_code)]
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

/// dyn 兼容的异步返回；不加 async-trait 依赖。
pub(crate) type HookFuture<'a> = Pin<Box<dyn Future<Output = Result<(), AgentError>> + Send + 'a>>;

/// 一个可挂到 loop 挂点上的功能（编译期 trait，不做运行时加载）。
pub(crate) trait Capability: Send + Sync {
    fn name(&self) -> &'static str;
    fn hooks(&self) -> &'static [HookPoint];
    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a>;
}

/// 1a 的占位登记：只声明功能与它关心的挂点，不接管行为（行为仍在原模块）。
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
        Box::pin(async { Ok(()) })
    }
}

const PRUNE_HOOKS: &[HookPoint] = &[HookPoint::Compaction];
const TODO_HOOKS: &[HookPoint] = &[HookPoint::ToolAfter];
const SPILL_HOOKS: &[HookPoint] = &[HookPoint::ToolResult, HookPoint::ToolAfter];
const REMINDER_HOOKS: &[HookPoint] = &[HookPoint::ToolAfter];
const JOBS_HOOKS: &[HookPoint] = &[HookPoint::Status];

static COMPACTION_CAP: CompactionCapability = CompactionCapability;
static PRUNE: Placeholder = Placeholder {
    name: "prune",
    hooks: PRUNE_HOOKS,
};
static INSTRUCTIONS_CAP: InstructionsCapability = InstructionsCapability;
static SKILL_CAP: SkillCapability = SkillCapability;
static TODO: Placeholder = Placeholder {
    name: "todo",
    hooks: TODO_HOOKS,
};
static ASK: Placeholder = Placeholder {
    name: "ask",
    hooks: &[],
};
static SPILL: Placeholder = Placeholder {
    name: "spill",
    hooks: SPILL_HOOKS,
};
static REMINDER: Placeholder = Placeholder {
    name: "reminder",
    hooks: REMINDER_HOOKS,
};
static JOBS: Placeholder = Placeholder {
    name: "jobs",
    hooks: JOBS_HOOKS,
};

/// 固定顺序的功能注册表；顺序=strategy §2 功能表的拆票顺序（compaction→…→jobs）。
pub(crate) struct CapabilityRegistry {
    capabilities: Vec<&'static dyn Capability>,
}

impl CapabilityRegistry {
    pub(crate) fn production() -> Self {
        Self {
            capabilities: vec![
                &COMPACTION_CAP,
                &PRUNE,
                &INSTRUCTIONS_CAP,
                &SKILL_CAP,
                &TODO,
                &ASK,
                &SPILL,
                &REMINDER,
                &JOBS,
            ],
        }
    }

    /// 按注册顺序派发；首个 Err 短路（之前的功能已执行完，不回滚）。
    pub(crate) async fn dispatch(&self, event: &HookEvent<'_>) -> Result<(), AgentError> {
        for capability in &self.capabilities {
            debug_assert!(!capability.name().is_empty());
            if capability.hooks().contains(&event.hook()) {
                capability.on_hook(event).await?;
            }
        }
        Ok(())
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
        | EventKind::ToolCall { turn, step, .. }
        | EventKind::ToolResult { turn, step, .. }
        | EventKind::TodoWrite { turn, step, .. }
        | EventKind::QuestionAsked { turn, step, .. }
        | EventKind::QuestionCancelled { turn, step, .. } => (*turn, *step),
        EventKind::CompactionStart { turn, .. } | EventKind::CompactionEnd { turn, .. } => {
            (*turn, 0)
        }
        EventKind::CompactionSummary { .. } | EventKind::CompactionPrune { .. } => return None,
    };
    Some(pair)
}
