//! Task 聚合：任务、Run、Attempt、报告和执行者写入的缺项。

use std::fs;
use std::path::{Path, PathBuf};

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
    /// 当前写入范围（已按 cwd 解析的绝对路径）；None = 只读调查任务（D85）。
    #[serde(default)]
    pub write_scope: Option<PathBuf>,
    /// 创建时冻结的 bash 开关；旧任务行按 false 读（D85）。
    #[serde(default)]
    pub bash: bool,
    /// 是否在新 git 工作树里做；旧任务行按 false 读（D86）。
    #[serde(default)]
    pub worktree: bool,
    /// 写集；只读任务固定为空。旧任务行没有这个字段时按空读。
    #[serde(default)]
    pub writes: Vec<String>,
    /// T024 不允许 CodeMode；字段随聚合保存，给后面的票用。
    pub code_mode: bool,
    pub runs: Vec<Run>,
    pub reports: Vec<Report>,
    /// 用户在任务对话里说过的话；旧任务行没有这个字段时按空读。
    #[serde(default)]
    pub inputs: Vec<Input>,
    /// 要求版本；建任务时记第一条，task_update 追加。旧任务行按空读。
    #[serde(default)]
    pub versions: Vec<RequirementVersion>,
    /// 用户接受记录；旧任务行按空读。接受不是阶段（03 篇 5.1、5.4）。
    #[serde(default)]
    pub acceptance: Option<Acceptance>,
    /// 执行者在起跑前查到的缺项；空表示任务包齐。
    pub missing: Vec<MissingItem>,
    pub created_at: i64,
}

/// 建任务时冻结的执行策略：写入范围、bash 开关、工作树（D85、D86）。
///
/// 写入范围和工作树来自 `task_accept` 的参数；bash 由服务在创建时从设置读出
/// 冻结，不是 `task_accept` 的字段。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaskPolicy {
    /// 写入范围原文：相对路径按任务 cwd 解析；None 或空白 = 只读调查任务。
    pub write_scope: Option<String>,
    /// 用户明说要工作树才开（D86）。
    pub worktree: bool,
    /// 创建时冻结的 bash 开关；默认关（D85）。
    pub bash: bool,
}

/// 解析并冻结后的建任务策略：写入范围已按 cwd 解析成绝对路径。
pub(crate) struct FrozenPolicy {
    pub(crate) write_scope: Option<PathBuf>,
    pub(crate) bash: bool,
    pub(crate) worktree: bool,
}

impl TaskPolicy {
    /// 按 cwd 解析写入范围。空白当没有范围；根目录、用户主目录本身拒绝。
    pub(crate) fn freeze(&self, cwd: &Path) -> Result<FrozenPolicy, TaskError> {
        Ok(FrozenPolicy {
            write_scope: resolve_write_scope(cwd, self.write_scope.as_deref())?,
            bash: self.bash,
            worktree: self.worktree,
        })
    }
}

/// 写入范围解析：相对路径按 cwd；存在的前缀 canonicalize，缺失的后缀原样接上
/// （范围可以指向还不存在的文件或目录）。空白等于没有范围。
///
/// 根目录和用户主目录本身拒绝：那是整台机器或整个家目录，不是一项任务该拿到的授权。
pub(crate) fn resolve_write_scope(
    cwd: &Path,
    raw: Option<&str>,
) -> Result<Option<PathBuf>, TaskError> {
    let Some(raw) = raw.filter(|raw| !raw.trim().is_empty()) else {
        return Ok(None);
    };
    let path = Path::new(raw);
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.join(path)
    };
    let resolved = canonicalize_existing_prefix(&absolute)?;
    if resolved.parent().is_none() {
        return Err(TaskError::WriteScopeIsRoot);
    }
    if home_directory().as_deref() == Some(resolved.as_path()) {
        return Err(TaskError::WriteScopeIsHome);
    }
    Ok(Some(resolved))
}

/// 用户主目录；取不到 HOME 时不做这一项校验。
fn home_directory() -> Option<PathBuf> {
    let home = PathBuf::from(std::env::var_os("HOME")?);
    Some(fs::canonicalize(&home).unwrap_or(home))
}

/// 存在的前缀 canonicalize，缺失的后缀接回去；和 tools 的 resolve 同语义。
fn canonicalize_existing_prefix(path: &Path) -> Result<PathBuf, TaskError> {
    let mut missing: Vec<std::ffi::OsString> = Vec::new();
    let mut current = path.to_path_buf();
    loop {
        match fs::canonicalize(&current) {
            Ok(mut real) => {
                for part in missing.iter().rev() {
                    real.push(part);
                }
                return Ok(real);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let Some(name) = current.file_name().map(std::ffi::OsStr::to_os_string) else {
                    return Err(TaskError::WriteScopeUnresolvable(
                        path.display().to_string(),
                    ));
                };
                missing.push(name);
                let Some(parent) = current.parent().map(Path::to_path_buf) else {
                    return Err(TaskError::WriteScopeUnresolvable(
                        path.display().to_string(),
                    ));
                };
                current = parent;
            }
            Err(error) => {
                return Err(TaskError::WriteScopeUnresolvable(format!(
                    "{}: {error}",
                    current.display()
                )));
            }
        }
    }
}

/// 一条要求版本：写入范围、时间、来源（D16）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequirementVersion {
    pub write_scope: Option<PathBuf>,
    pub at: i64,
    pub source: VersionSource,
}

/// 一条要求版本是怎么来的。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VersionSource {
    /// 任务受理（task_accept）时定下的第一条要求。
    TaskAccept,
    /// 用户在 Kind 对话里同意扩大写入范围（task_update）。
    UserAgreed,
}

/// 用户对某份结果的接受：绑所看的要求版本和报告（03 篇 5.4）。
///
/// 接受不是 Task 阶段，记下来不改阶段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Acceptance {
    /// 接受时看的要求版本号（1 起）；旧任务行没有版本记录，记 0。
    pub version: u32,
    pub report_id: ReportId,
    pub at: i64,
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
    /// 重启打断后，用户确认接续。
    Resume,
    /// 用户在 Kind 对话里同意改要求（task_update）：原会话开新 Attempt。
    RequirementChanged,
}

/// 一条用户输入；来源目前只有用户本人。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub id: InputId,
    pub at: i64,
    pub source: InputSource,
    pub text: String,
    /// 和这句话一起发来的粘贴段（照原文）；旧任务行没有这个字段时按空读。
    #[serde(default)]
    pub pastes: Vec<String>,
    /// 和这句话一起发来的附件；旧任务行没有这个字段时按空读。
    ///
    /// 只记附件仓库里的 id、名字和字节数；投递时再按 id 解析只读副本路径。
    #[serde(default)]
    pub attachments: Vec<InputAttachment>,
}

/// 用户在任务对话里说的一句话：文字、粘贴段、附件至少有一项。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewInput {
    pub text: String,
    pub pastes: Vec<String>,
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
    /// 当前要求版本号（1 起）；旧任务行没有版本记录时为 0。
    pub fn requirement_version(&self) -> u32 {
        u32::try_from(self.versions.len()).unwrap_or(u32::MAX)
    }

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

    /// 最近的 Attempt 以 interrupted 结束、之后没有新的 Attempt：任务「被打断了」。
    pub fn is_interrupted(&self) -> bool {
        self.latest_attempt()
            .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Interrupted))
    }

    /// 任务曾经被打断过：任意一次 Attempt 以 interrupted 结束。
    pub fn was_interrupted(&self) -> bool {
        self.runs
            .iter()
            .flat_map(|run| run.attempts.iter())
            .any(|attempt| attempt.end == Some(AttemptEnd::Interrupted))
    }
}
