//! Task 聚合：任务、Run、Attempt、报告和执行者写入的缺项。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::TaskError;
use crate::id::{AttemptId, ConversationId, InputId, ReportId, RunId, TaskId};

/// 创建任务时冻结的模型选择；之后改默认模型不影响这个任务。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelChoice {
    pub connection: String,
    pub model: String,
    pub effort_wanted: String,
}

/// 一个 Task 的完整聚合，整行存在 task 表。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub title: String,
    pub requirement: String,
    pub dod: String,
    pub cwd: PathBuf,
    /// 发起任务的 Kind 会话。
    pub conversation: ConversationId,
    pub model: ModelChoice,
    /// 允许这个任务用的工具名单。
    pub tools: Vec<String>,
    /// T024 的写集固定为空；写任务的票才允许非空。
    pub writes: Vec<String>,
    /// T024 不允许 CodeMode；字段随聚合保存，给后面的票用。
    pub code_mode: bool,
    pub runs: Vec<Run>,
    pub reports: Vec<Report>,
    /// 用户在任务对话里说过的话；旧任务行没有这个字段时按空读。
    #[serde(default)]
    pub inputs: Vec<Input>,
    /// 执行者在起跑前查到的缺项；空表示任务包齐。
    pub missing: Vec<MissingItem>,
    pub created_at: i64,
}

/// 一次执行推进；首个 Attempt 会开出第一个 Run。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Run {
    pub id: RunId,
    pub started_at: i64,
    pub attempts: Vec<Attempt>,
}

/// 一次执行或恢复尝试；会话 id 在会话真的开出来之后才写回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Attempt {
    pub id: AttemptId,
    pub conversation: Option<ConversationId>,
    /// 这次 Attempt 是怎么开始的；旧任务行没有这个字段时按派发读。
    #[serde(default)]
    pub trigger: AttemptTrigger,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end: Option<AttemptEnd>,
}

/// 一次 Attempt 是怎么开始的。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptTrigger {
    /// 任务派发（launch）。
    #[default]
    Launch,
    /// 用户在任务对话里说话，接着做。
    UserMessage,
}

/// 一条用户输入；来源目前只有用户本人。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub id: InputId,
    pub at: i64,
    pub source: InputSource,
    pub text: String,
    /// 和这句话一起发来的附件；旧任务行没有这个字段时按空读。
    ///
    /// 只记附件仓库里的 id、名字和字节数；投递时再按 id 解析只读副本路径。
    #[serde(default)]
    pub attachments: Vec<InputAttachment>,
}

/// 一条输入里引用的附件（规则和 Kind 对话一样）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InputAttachment {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

/// 输入是谁说的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InputSource {
    User,
}

/// 说一句的结果：记下的输入，和没在跑时叫醒的新 Attempt。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SayOutcome {
    pub input_id: InputId,
    /// 上一 Attempt 已结束时新开的 Attempt；还在跑或从没开跑时为 None。
    pub new_attempt: Option<AttemptId>,
}

/// Attempt 的结束原因。时间由调用方传入，库内不读时钟。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttemptEnd {
    /// ThinAgent 报过终态，Attempt 随报告结束。
    /// 兼容 T024 落库的 `ended`：只读旧值，新值写 `reported`。
    #[serde(alias = "ended")]
    Reported,
    /// 立即停；停了之后这次 Attempt 不再接受报告。
    Stopped,
    /// 重启打断，留给恢复链收尾。
    Interrupted,
    /// 崩溃留下的、没开成会话的 Attempt 被收尾。
    NotOpened,
}

/// ThinAgent 报的终态；`Waiting` 是执行者在 turn 结束时合成的，不是模型报的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReportState {
    Ready,
    Blocked,
    Failed,
    Waiting,
}

impl ReportState {
    /// 终态是 ThinAgent 报的三种；`Waiting` 是执行者 turn 结束合成的，不是终态。
    pub(crate) fn is_terminal(self) -> bool {
        matches!(self, Self::Ready | Self::Blocked | Self::Failed)
    }
}

/// 报告是谁写的：缺项报告由执行者按任务包检查的事实写，终态来自 ThinAgent 的
/// task_report，waiting 和开会话、派发失败由执行者合成。relay 用它区分「没开跑」
/// 和「卡住了」，不让卡片去读摘要文字。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReportOrigin {
    Package,
    Agent,
    Executor,
}

/// 一份任务报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub id: ReportId,
    pub attempt_id: AttemptId,
    pub state: ReportState,
    pub origin: ReportOrigin,
    pub summary: String,
    pub question: Option<String>,
    pub at: i64,
}

/// 任务包缺的一项；执行者写什么，store 就存什么，不改写成卡片状态。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MissingItem(String);

impl MissingItem {
    pub fn new(text: impl Into<String>) -> Result<Self, TaskError> {
        let text = text.into();
        if text.trim().is_empty() {
            return Err(TaskError::EmptyMissing);
        }
        Ok(Self(text))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// 建任务需要的字段；写集和 CodeMode 在 T024 固定为空/否。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTask {
    pub title: String,
    pub requirement: String,
    pub dod: String,
    pub cwd: PathBuf,
    pub conversation: ConversationId,
    pub model: ModelChoice,
    pub tools: Vec<String>,
}

impl NewTask {
    pub fn validate(&self) -> Result<(), TaskError> {
        let title_length = self.title.chars().count();
        if !(1..=200).contains(&title_length) {
            return Err(TaskError::InvalidTitle(title_length));
        }
        if self.requirement.trim().is_empty() {
            return Err(TaskError::EmptyRequirement);
        }
        if self.dod.trim().is_empty() {
            return Err(TaskError::EmptyDod);
        }
        if self.cwd.as_os_str().is_empty() {
            return Err(TaskError::EmptyCwd);
        }
        if self.conversation.as_str().trim().is_empty() {
            return Err(TaskError::EmptyConversation);
        }
        if self.model.connection.trim().is_empty() {
            return Err(TaskError::IncompleteModel("connection"));
        }
        if self.model.model.trim().is_empty() {
            return Err(TaskError::IncompleteModel("model"));
        }
        Ok(())
    }
}

/// 写一份报告需要的字段。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewReport {
    pub state: ReportState,
    pub origin: ReportOrigin,
    pub summary: String,
    pub question: Option<String>,
}

impl NewReport {
    pub fn validate(&self) -> Result<(), TaskError> {
        if self.summary.trim().is_empty() {
            return Err(TaskError::EmptySummary);
        }
        if let Some(question) = &self.question {
            if question.trim().is_empty() {
                return Err(TaskError::EmptyQuestion);
            }
            let length = question.chars().count();
            if length > 1000 {
                return Err(TaskError::QuestionTooLong(length));
            }
        }
        Ok(())
    }
}

impl Task {
    pub fn has_attempt(&self) -> bool {
        self.runs.iter().any(|run| !run.attempts.is_empty())
    }

    pub fn latest_attempt(&self) -> Option<&Attempt> {
        self.runs
            .iter()
            .rev()
            .flat_map(|run| run.attempts.iter().rev())
            .next()
    }

    /// 最近一份报告只从最新的 Attempt 取：旧 Attempt 的迟到报告只当历史。
    pub fn latest_report(&self) -> Option<&Report> {
        let attempt = self.latest_attempt()?;
        self.reports
            .iter()
            .rev()
            .find(|report| report.attempt_id == attempt.id)
    }

    pub fn attempt(&self, id: AttemptId) -> Option<&Attempt> {
        self.runs
            .iter()
            .flat_map(|run| run.attempts.iter())
            .find(|attempt| attempt.id == id)
    }
}
