//! 日历库：约定、版本、运行记录、规则计算和复核纯逻辑。
//!
//! 只依赖 rusqlite/serde/rrule/chrono/chrono-tz；不依赖 service/agent/tasks/session/memory。
//! 库文件是 <data-dir>/calendar.sqlite，schema 版本、迁移、单写者、事务都在 store。

pub mod error;
pub mod review;
pub mod schedule;
pub mod store;
pub mod types;

pub use error::CalendarError;
pub use review::{merge_missed, merged_reason, review};
pub use store::{CalendarStore, SCHEMA_VERSION};
pub use types::{
    Appointment, AppointmentPatch, AppointmentStatus, AppointmentVersion, Binding, BoundTaskFacts,
    Clock, CycleSpec, Missed, NewAppointment, NewRun, ProgressSignature, Review, ReviewFacts, Run,
    RunResult, Schedule, TriggerKind, WatchOutcome, WatchSource,
};
