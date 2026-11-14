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
    #[error("启动 id 不能为空")]
    EmptyBootId,
    #[error("模型选择缺 {0}")]
    IncompleteModel(&'static str),
    #[error("任务包缺项描述不能为空")]
    EmptyMissing,
    #[error("任务对话里的输入不能为空")]
    EmptyInput,
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
    #[error("写入范围不能是文件系统根目录")]
    WriteScopeIsRoot,
    #[error("写入范围不能是用户主目录本身")]
    WriteScopeIsHome,
    #[error("写入范围解析不了：{0}")]
    WriteScopeUnresolvable(String),
    #[error("task_update 的写入范围不能为空")]
    EmptyWriteScope,
    #[error("写入范围不在工作树对应的仓库里：{0}")]
    WriteScopeOutsideWorktree(String),
    #[error("任务还没有可以接受的结果报告")]
    NoReportToAccept,
    #[error("要求改过了：这份是第 {report} 版要求的结果，没有记接受")]
    ReportNotCurrentVersion { report: u32, current: u32 },
    /// T054：找不这份报告，或它不是 ready。
    #[error("这份结果不能接受：它不是 ready 报告")]
    ReportNotReady,
    /// T054：同一任务后面又交了新结果。
    #[error("已经变了：这不是它最新交的结果，没有记接受")]
    ReportNotLatest,
    /// T054：门任务自己不能接受。
    #[error("这是门任务：核查结果由你在门卡上判，不接受")]
    GateTaskCannotAccept,
    /// T054：有门任务的门还没对这份报告判通过。
    #[error("核查还没判通过，没有记接受")]
    GateNotPassed,
    #[error("任务图至少要两个子任务，实际 {0} 个")]
    GraphTooFewSubtasks(usize),
    #[error("任务图第 {subtask} 项的 after 指向本次提议之外的子任务：{after}")]
    GraphAfterOutOfRange { subtask: usize, after: usize },
    #[error("任务图的先后关系有环")]
    GraphHasCycle,
    #[error("任务图里没有先后的第 {a} 项和第 {b} 项写入范围重叠：{scope_a} 与 {scope_b}")]
    GraphWriteScopeOverlap {
        a: usize,
        b: usize,
        scope_a: String,
        scope_b: String,
    },
    #[error("总任务不跑执行，不能开 Attempt")]
    GraphRootCannotRun,
    #[error("这个任务不是任务图的总任务")]
    NotGraphRoot,
    #[error("这张任务图已经确认过了")]
    GraphAlreadyConfirmed,
    #[error("任务图第 {subtask} 项是里程碑，必须是只读（不能有写入范围）")]
    GraphMilestoneHasWriteScope { subtask: usize },
    #[error("任务图第 {subtask} 项是里程碑，不能开工作树")]
    GraphMilestoneHasWorktree { subtask: usize },
    #[error("任务图第 {subtask} 项是里程碑，after 必须恰好一项（把关的那个任务），实际 {count} 项")]
    GraphMilestoneAfterNotOne { subtask: usize, count: usize },
    #[error("任务图第 {subtask} 项是里程碑，不能把关另一个里程碑（不能有门链）")]
    GraphMilestoneAfterIsMilestone { subtask: usize },
    #[error("任务图第 {subtask} 项的 after 指向了里程碑（里程碑只能由被它把关的任务单独等）")]
    GraphMilestoneCannotBeDependency { subtask: usize },
    #[error("任务图第 {first} 项和第 {second} 项都想给同一个任务当里程碑；一个任务最多一个门")]
    GraphTwoMilestonesForGuarded { first: usize, second: usize },
    #[error("这个任务不是里程碑门")]
    NotMilestoneGate,
    #[error("这张任务图已经取消了")]
    GraphCancelled,
    #[error("门的最近报告不是这份，或者候选已经变了")]
    GateReportChanged,
}
