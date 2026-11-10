//! 任务存储的错误。

use thiserror::Error;

use crate::task::ReportState;

/// tasks crate 的错误；一个失败只有一个 owner。
#[derive(Debug, Error)]
pub enum TaskError {
    #[error("任务数据目录或文件不可用: {0}")]
    Io(#[from] std::io::Error),
    #[error("SQLite 错误: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("任务数据 JSON 错误: {0}")]
    Json(#[from] serde_json::Error),
    #[error("任务库 schema 版本 {found} 比本机 {supported} 新，拒绝打开")]
    SchemaTooNew { found: u64, supported: u64 },
    #[error("任务库 schema 版本 {found} 不受支持（本机 {supported}）")]
    SchemaUnsupported { found: u64, supported: u64 },
    #[error("任务库 schema 版本记录损坏: {0}")]
    CorruptSchema(String),
    #[error("SQLite 没进入 WAL/FULL 持久化模式: {0}")]
    StorageConfig(String),
    #[error("任务库连接锁已中毒")]
    Poisoned,
    #[error("任务不存在: {0}")]
    TaskNotFound(String),
    #[error("任务 {task} 的 Attempt {attempt} 不存在")]
    AttemptNotFound { task: String, attempt: u64 },
    #[error("这次 Attempt 已结束（任务 {task} 的第 {attempt} 次）")]
    AttemptEnded { task: String, attempt: u64 },
    #[error("任务 {task} 还有没结束的 Attempt {attempt}，不能开新的")]
    AttemptStillOpen { task: String, attempt: u64 },
    #[error("Attempt {attempt} 已经绑定了会话 {existing}，不能再改成 {requested}")]
    AttemptSessionConflict {
        attempt: u64,
        existing: String,
        requested: String,
    },
    #[error("标题长度必须在 1–200 字之间，实际 {0} 字")]
    InvalidTitle(usize),
    #[error("任务要求不能为空")]
    EmptyRequirement,
    #[error("DoD 不能为空")]
    EmptyDod,
    #[error("工作目录不能为空")]
    EmptyCwd,
    #[error("会话 id 不能为空")]
    EmptyConversation,
    #[error("受理幂等键不能为空")]
    EmptyAcceptKey,
    #[error("模型选择缺 {0}")]
    IncompleteModel(&'static str),
    #[error("任务包缺项描述不能为空")]
    EmptyMissing,
    #[error("报告摘要不能为空")]
    EmptySummary,
    #[error("报告问题不能为空（没有问题时留空）")]
    EmptyQuestion,
    #[error("报告问题超过 1000 字，实际 {0} 字")]
    QuestionTooLong(usize),
    #[error("Attempt {attempt} 已经报过 {existing:?}，不能再报 {requested:?}")]
    ReportConflict {
        attempt: u64,
        existing: ReportState,
        requested: ReportState,
    },
    #[error("effect 行内容与键不一致: {kind}/{key}")]
    CorruptEffect { kind: &'static str, key: String },
}
