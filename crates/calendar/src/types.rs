//! 日历库的领域类型：约定、版本、运行记录、复核事实和结果。

use serde::{Deserialize, Serialize};

/// 注入时钟：毫秒时间戳；生产用系统时钟，测试全用假时钟。
pub type Clock = std::sync::Arc<dyn Fn() -> i64 + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppointmentStatus {
    Active,
    Paused,
    Ended,
    Cancelled,
}

impl AppointmentStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Active => "active",
            Self::Paused => "paused",
            Self::Ended => "ended",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "active" => Some(Self::Active),
            "paused" => Some(Self::Paused),
            "ended" => Some(Self::Ended),
            "cancelled" => Some(Self::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    /// 绑 Kind 根会话；正文原话交给 Kind。
    Kind,
    /// 绑一个任务；循环约定用。
    Task(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchSource {
    Url(String),
    File(String),
    TaskFacts(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CycleSpec {
    pub task_id: String,
    pub max_rounds: Option<u32>,
}

/// 规则：RRULE 正文 + IANA 时区 + 起点（DTSTART 等价物，毫秒时间戳）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    pub rule: String,
    pub timezone: String,
    pub start_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriggerKind {
    Due,
    Manual,
    Changed,
    Report,
}

impl TriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Due => "due",
            Self::Manual => "manual",
            Self::Changed => "changed",
            Self::Report => "report",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "due" => Some(Self::Due),
            "manual" => Some(Self::Manual),
            "changed" => Some(Self::Changed),
            "report" => Some(Self::Report),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RunResult {
    Queued,
    Sent,
    Skipped,
    Failed,
}

impl RunResult {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Sent => "sent",
            Self::Skipped => "skipped",
            Self::Failed => "failed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "queued" => Some(Self::Queued),
            "sent" => Some(Self::Sent),
            "skipped" => Some(Self::Skipped),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }

    /// 结果只往前走：排着 → 已送/没送成/跳过；没送成 → 已送；其余都不许回头。
    pub fn can_move_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Sent | Self::Skipped | Self::Failed)
                | (Self::Failed, Self::Sent)
                | (Self::Sent, Self::Sent)
                | (Self::Skipped, Self::Skipped)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Appointment {
    pub id: String,
    pub status: AppointmentStatus,
    pub version: u32,
    pub title: String,
    pub condition: String,
    pub schedule: Schedule,
    pub binding: Binding,
    pub watch: Option<WatchSource>,
    pub cycle: Option<CycleSpec>,
    pub next_trigger_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppointmentVersion {
    pub appointment_id: String,
    pub version: u32,
    pub title: String,
    pub condition: String,
    pub schedule: Schedule,
    pub binding: Binding,
    pub watch: Option<WatchSource>,
    pub cycle: Option<CycleSpec>,
    pub reason: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewAppointment {
    pub title: String,
    pub condition: String,
    pub schedule: Schedule,
    pub binding: Binding,
    pub watch: Option<WatchSource>,
    pub cycle: Option<CycleSpec>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppointmentPatch {
    pub title: Option<String>,
    pub condition: Option<String>,
    pub schedule: Option<Schedule>,
    pub binding: Option<Binding>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch: Option<Option<WatchSource>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle: Option<Option<CycleSpec>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRun {
    pub appointment_id: String,
    pub nominal_at: i64,
    pub manual: bool,
    pub trigger: TriggerKind,
    pub reason: String,
    pub merged_count: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Run {
    pub id: i64,
    pub appointment_id: String,
    pub nominal_at: i64,
    pub manual: bool,
    pub trigger: TriggerKind,
    pub result: RunResult,
    pub reason: String,
    pub merged_count: u32,
    pub created_at: i64,
    pub updated_at: i64,
    pub sent_turn: Option<i64>,
}

/// 绑定的任务现在是什么样（由服务侧从任务事实填，日历库不依赖 tasks）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoundTaskFacts {
    pub exists: bool,
    pub graph_cancelled: bool,
    pub closed: bool,
    pub running: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchOutcome {
    Failed(String),
    Unchanged,
    Changed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewFacts {
    pub status: AppointmentStatus,
    pub bound_task: Option<BoundTaskFacts>,
    pub kind_busy: bool,
    pub global_paused: bool,
    pub watch: Option<WatchOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Review {
    /// 不起运行（已取消/已结束）。
    NoRun,
    /// 绑定任务已关闭：约定结束。
    End,
    /// 记一条跳过，正文是页面上原样显示的固定说法。
    Skip(String),
    /// 排着（全局暂停等）；开闸后合并。
    Queue(String),
    /// 没送成（重试照效果表退避）。
    Fail(String),
    /// 送 Kind。
    Send,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ProgressSignature {
    pub state: String,
    pub failed_checks: Vec<(String, i32)>,
    pub changed_files: Vec<String>,
}

impl ProgressSignature {
    pub fn same_progress(&self, other: &Self) -> bool {
        self == other
    }
}

/// 错过合并的结果：只补最近一次，并记合并了几次。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Missed {
    pub latest: Option<i64>,
    pub merged: u32,
}
