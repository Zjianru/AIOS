//! 任务领域：Task/Run/Attempt/报告、阶段派生、席位计数、SQLite 存储和持久效果表。
//!
//! 边界：只依赖 rusqlite/serde/serde_json，不依赖 agent/session；会话 id 是不透明字符串。
//! 时间一律由调用方传入 unix 毫秒，库内不读时钟。

#[cfg(test)]
mod usage_wait_tests;

#[cfg(test)]
mod origin_tests;

#[cfg(test)]
#[path = "cancel_tests.rs"]
mod cancel_tests;

mod effect;
mod error;
mod id;
mod ledger;
mod project;
mod slots;
mod stage;
mod store;
mod task;

pub use effect::{
    EffectClaim, EffectHealth, EffectKind, EffectPayload, EffectState, FAILURES_BEFORE_ERROR,
};
pub use error::TaskError;
pub use id::{AttemptId, BootId, ConversationId, InputId, ReportId, RunId, TaskId};
pub use ledger::{LedgerNote, LedgerNoteCounts, LedgerNoteKind, NewLedgerNote};
pub use project::{NewProject, PROJECTS_DB_FILE, Project, ProjectError, ProjectStore};
pub use slots::{Later, SlotKind, WorkerSlots};
pub use stage::Stage;
pub use store::{BootScan, GateJudgeOutcome, SCHEMA_VERSION, StopAttempt, Store};
pub use task::{
    AcceptBatch, AcceptOutcome, Acceptance, AppointmentOrigin, AppointmentRole, Arrangement,
    Attempt, AttemptEnd, AttemptStop, AttemptTrigger, CANCELLED_MESSAGE, Cancellation,
    CancellationReason, CancellationSource, Confirmation, ContinueCause, GateCandidate, GateCheck,
    GateCheckOutcome, GateCommand, GateConclusion, GateVerdict, Handoff, Input, InputAttachment,
    InputObjectRef, InputSource, MilestoneGate, MissingItem, ModelChoice, Need, NewGraph, NewInput,
    NewReport, NewSubtask, NewTask, ProgramGateOutcome, QuoteSource, Report, ReportOrigin,
    ReportState, RequirementVersion, RoundEnd, Run, SayOutcome, Task, TaskCancellation, TaskPolicy,
    VerdictSource, VersionSource, Worktree, validate_gate_commands,
};
