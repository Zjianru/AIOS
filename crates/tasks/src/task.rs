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

/// T052：任务图里一个子任务等的一个兄弟子任务；开跑那一刻解析出交接。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Need {
    /// 上游兄弟任务。
    pub from: TaskId,
    /// 开跑时解析的上游 ready 报告事实；还没解析就是 None（旧任务行按 None 读）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub handoff: Option<Handoff>,
}

/// T052：上游交给下游的「上游结果」事实（04 篇 3.5、3.6）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handoff {
    pub report_id: ReportId,
    pub requirement_version: u32,
    pub summary: String,
}

/// T053：里程碑门（空标记）：一个只读核查子任务；把关谁 = 它唯一的 needs。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MilestoneGate {}

/// T053：门判定的候选 = 被把关任务当前最新 ready 报告里的一份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateCandidate {
    pub report_id: ReportId,
    pub requirement_version: u32,
}

/// T053：用户判定结论。只有用户在门结果卡上点。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GateConclusion {
    Pass,
    NeedsFix,
}

/// T053：一条判定记录，按时间追加（旧记录留历史，04 篇 6.1）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GateVerdict {
    /// 门的那份 ready 报告（判定作用在报告上）。
    pub gate_report: ReportId,
    /// 被把关任务被判的那份候选。
    pub candidate: GateCandidate,
    pub conclusion: GateConclusion,
    /// 用户原话（需修正时；原样存，不截断不改写）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: i64,
}

/// T053：task_continue 的原因，由服务按事实认。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContinueCause {
    /// 当前候选被判「需修正」。
    GateNeedsFix {
        gate_report: ReportId,
        candidate: GateCandidate,
    },
    /// 上游有新结果，下游要按新结果核对（D95）。
    UpstreamChanged { upstream_report: ReportId },
    /// 最近报告卡住、失败、等报告（重跑门也走这条）。
    AfterReport { report: ReportId },
}

/// T052：总任务上的安排记录：图版本与本票唯一一次确认。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Arrangement {
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<Confirmation>,
}

/// T052：用户的确认记录（只由确认卡上的 card-act 写）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Confirmation {
    pub version: u32,
    pub at: i64,
}

/// T052：Kind 提的一张任务图：总任务 + 至少两个子任务 + 先后边。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGraph {
    pub root: NewTask,
    pub subtasks: Vec<NewSubtask>,
}

/// T052：图里的一个子任务；after 是同一提议里其他子任务的下标（0 起）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSubtask {
    pub task: NewTask,
    pub policy: TaskPolicy,
    pub after: Vec<usize>,
    /// T053：里程碑（只读核查门）；after 恰好一项，指向它把关的任务。
    pub milestone: bool,
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
    /// 第一次 Attempt 起跑前开出来的工作树；之后每个 Attempt 都用它。旧任务行按 None 读。
    #[serde(default)]
    pub worktree_state: Option<Worktree>,
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
    /// T052：任务图里这个子任务的总任务；单派任务和总任务为 None。旧任务行按 None 读。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<TaskId>,
    /// T052：这个子任务等哪些兄弟子任务（开跑时把交接解析进每项）。旧任务行按空读。
    #[serde(default)]
    pub needs: Vec<Need>,
    /// T052：总任务上的安排（图版本 + 用户确认）；子任务和单派任务为 None。旧任务行按 None 读。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangement: Option<Arrangement>,
    /// T053：里程碑门（空标记）；只有门任务有。旧任务行按 None 读。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<MilestoneGate>,
    /// T053：用户判定记录（只有门任务会追加）。旧任务行按空读。
    #[serde(default)]
    pub verdicts: Vec<GateVerdict>,
    /// 执行者在起跑前查到的缺项；空表示任务包齐。
    pub missing: Vec<MissingItem>,
    pub created_at: i64,
}

/// 一次任务开出来的 git 工作树：第一次 Attempt 前落下，之后沿用，不自动合并/清理（D86）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Worktree {
    /// 工作树根（绝对、canonical）：<数据目录>/worktrees/<taskId>。
    pub root: PathBuf,
    /// 原仓库根（绝对、canonical）；写入范围按仓库内的相对位置换算。
    pub repo: PathBuf,
    /// ThinAgent 的 cwd（绝对、canonical）：工作树里和原 cwd 对应的子目录。
    pub cwd: PathBuf,
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
    /// T053：开跑时实际拿到的上游报告，顺序和 Task.needs 一一对应；旧任务行按空读。
    #[serde(default)]
    pub handoffs: Vec<Handoff>,
    pub started_at: i64,
    pub ended_at: Option<i64>,
    pub end: Option<AttemptEnd>,
}

/// 一次 Attempt 是怎么开始的。
///
/// T053 起不再 Copy：门重检和 Kind 接续带候选/原因事实。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
    /// 服务层面放门：被把关任务有新 ready 候选，门自动重检。
    GateRecheck { candidate: GateCandidate },
    /// Kind 用 task_continue 让任务在原要求里接着做。
    KindContinue {
        cause: ContinueCause,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
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
    /// 这份报告答复的要求版本（1 起）；旧任务行没有这个字段时按 0 读。
    /// 接受只接受当前版本的报告。
    #[serde(default)]
    pub version: u32,
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

    /// ThinAgent 实际使用的工作目录：开了工作树的用工作树里对应的子目录。
    pub fn effective_cwd(&self) -> &Path {
        match &self.worktree_state {
            Some(worktree) => &worktree.cwd,
            None => &self.cwd,
        }
    }

    /// 这次执行真正生效的写入范围：开着工作树时按仓库内的相对位置换算。
    ///
    /// 范围不在原仓库里时保持原样：它不是从原 cwd 换出来的，工作树里也没有对应位置。
    pub fn effective_write_scope(&self) -> Option<PathBuf> {
        let scope = self.write_scope.as_deref()?;
        let Some(worktree) = &self.worktree_state else {
            return Some(scope.to_path_buf());
        };
        match scope.strip_prefix(&worktree.repo) {
            Ok(relative) => Some(worktree.root.join(relative)),
            Err(_) => Some(scope.to_path_buf()),
        }
    }

    /// T052：总任务（有安排记录、不跑执行）。
    pub fn is_graph_root(&self) -> bool {
        self.arrangement.is_some()
    }

    /// T053：里程碑门任务（只读核查，结果由用户判）。
    pub fn is_gate(&self) -> bool {
        self.gate.is_some()
    }

    /// T053：门对某份门报告已有的判定（重复点同一张卡返回已有判定）。
    pub fn verdict_for_gate_report(&self, report_id: &ReportId) -> Option<&GateVerdict> {
        self.verdicts
            .iter()
            .rev()
            .find(|verdict| &verdict.gate_report == report_id)
    }

    /// T053：某份候选报告是否被本门判过「通过」。
    pub fn passed(&self, report_id: &ReportId) -> bool {
        self.verdicts.iter().rev().any(|verdict| {
            &verdict.candidate.report_id == report_id && verdict.conclusion == GateConclusion::Pass
        })
    }

    /// T053：某份候选报告最近一次判定是不是「需修正」。
    pub fn needs_fix(&self, report_id: &ReportId) -> Option<&GateVerdict> {
        self.verdicts.iter().rev().find(|verdict| {
            &verdict.candidate.report_id == report_id
                && verdict.conclusion == GateConclusion::NeedsFix
        })
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
