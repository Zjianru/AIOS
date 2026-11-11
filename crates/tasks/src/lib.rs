//! 任务领域：Task/Run/Attempt/报告、阶段派生、席位计数、SQLite 存储和持久效果表。
//!
//! 边界：只依赖 rusqlite/serde/serde_json，不依赖 agent/session；会话 id 是不透明字符串。
//! 时间一律由调用方传入 unix 毫秒，库内不读时钟。

mod effect;
mod error;
mod id;
mod slots;
mod stage;
mod store;
mod task;

pub use effect::{
    EffectClaim, EffectHealth, EffectKind, EffectPayload, EffectState, FAILURES_BEFORE_ERROR,
};
pub use error::TaskError;
pub use id::{AttemptId, BootId, ConversationId, InputId, ReportId, RunId, TaskId};
pub use slots::{Later, SlotKind, WorkerSlots};
pub use stage::Stage;
pub use store::{BootScan, SCHEMA_VERSION, StopAttempt, Store};
pub use task::{
    Acceptance, Attempt, AttemptEnd, AttemptTrigger, Input, InputAttachment, InputSource,
    MissingItem, ModelChoice, NewInput, NewReport, NewTask, Report, ReportOrigin, ReportState,
    RequirementVersion, Run, SayOutcome, Task, TaskPolicy, VersionSource,
};
