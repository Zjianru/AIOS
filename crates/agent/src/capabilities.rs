//! loop 的固定挂点与功能注册表（T006.1a 骨架，行为不变）。
//!
//! 挂点照 strategy §1 的 DSH 列表；Rust 接法是编译期固定 trait 列表 + 固定顺序注册表，
//! 不允许运行时加载。1a 只登记功能与它关心的挂点，功能实现仍留在原模块（1b–1e 再移）。

use session::EventKind;

/// loop 的固定挂点（对应 strategy §1 的六行，工具三阶段拆开计数）。
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
    /// 工具执行后：落盘收尾/其它后处理。
    ToolAfter,
    /// 状态变化：对外推送（服务接入后使用）。
    Status,
    /// 本轮要停：对外推送收尾（服务接入后使用）。
    TurnStopping,
}

/// 挂点上下文：1a 只带 turn/step；后续票按需扩展（不改变挂点集合）。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HookContext {
    pub turn: u64,
    pub step: u64,
}

impl HookContext {
    pub(crate) fn new(turn: u64, step: u64) -> Self {
        Self { turn, step }
    }
}

/// 一个可挂到 loop 挂点上的功能（编译期 trait，不做运行时加载）。
pub(crate) trait Capability: Send + Sync {
    fn name(&self) -> &'static str;
    fn hooks(&self) -> &'static [HookPoint];
    fn on_hook(&self, _hook: HookPoint, _context: HookContext) {}
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
}

const COMPACTION_HOOKS: &[HookPoint] = &[
    HookPoint::PreStep,
    HookPoint::Request,
    HookPoint::RequestError,
];
const PRUNE_HOOKS: &[HookPoint] = &[HookPoint::PreStep];
const INSTRUCTIONS_HOOKS: &[HookPoint] = &[
    HookPoint::PreStep,
    HookPoint::SessionEvent,
    HookPoint::ToolBefore,
];
const SKILL_HOOKS: &[HookPoint] = &[HookPoint::PreStep, HookPoint::ToolBefore];
const TODO_HOOKS: &[HookPoint] = &[HookPoint::ToolAfter];
const SPILL_HOOKS: &[HookPoint] = &[HookPoint::ToolResult, HookPoint::ToolAfter];
const REMINDER_HOOKS: &[HookPoint] = &[HookPoint::ToolAfter];
const JOBS_HOOKS: &[HookPoint] = &[HookPoint::Status];

static COMPACTION: Placeholder = Placeholder {
    name: "compaction",
    hooks: COMPACTION_HOOKS,
};
static PRUNE: Placeholder = Placeholder {
    name: "prune",
    hooks: PRUNE_HOOKS,
};
static INSTRUCTIONS: Placeholder = Placeholder {
    name: "instructions",
    hooks: INSTRUCTIONS_HOOKS,
};
static SKILL: Placeholder = Placeholder {
    name: "skill",
    hooks: SKILL_HOOKS,
};
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
                &COMPACTION,
                &PRUNE,
                &INSTRUCTIONS,
                &SKILL,
                &TODO,
                &ASK,
                &SPILL,
                &REMINDER,
                &JOBS,
            ],
        }
    }

    /// 按注册顺序把挂点事件分发给关心它的功能；1a 全是 no-op。
    pub(crate) fn dispatch(&self, hook: HookPoint, context: HookContext) {
        for capability in &self.capabilities {
            debug_assert!(!capability.name().is_empty());
            if capability.hooks().contains(&hook) {
                capability.on_hook(hook, context);
            }
        }
    }
}

/// 从一条日志事件取（turn, step）作为挂点上下文；没有 turn/step 的事件不派发。
pub(crate) fn event_context(kind: &EventKind) -> Option<HookContext> {
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
    Some(HookContext::new(pair.0, pair.1))
}
