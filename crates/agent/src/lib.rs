//! agent loop：一轮多步的驱动、收件箱，和 Agent 的 6 个入口。
//!
//! 机制照 DSH 的 packages/core/agent-loop（strategy 第 1 节）。

mod ask_capability;
mod capabilities;
mod compaction;
mod compaction_capability;
mod driver;
mod error;
mod inbox;
mod instructions;
mod instructions_capability;
mod jobs_capability;
mod prune;
mod reminder_capability;
mod skill_capability;
mod skill_catalog;
mod spill_capability;
mod todo_capability;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use model::{DeepSeek, Usage, UserMessage};
use session::{SessionId, SessionStore, TurnEndReason};
use tools::ToolSet;

pub use error::AgentError;
pub use inbox::Delivery;

/// agent 的运行配置。默认值照 DSH 默认组合：agent-loop 的并发上限、
/// compaction-basic 的阈值/保留/预留 token、bundle/base 的 spill 预算。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentConfig {
    /// 一次 step 里并行安全工具调用的并发上限；设成 1 就是串行。DSH 默认 10。
    pub max_parallel_tool_calls: usize,
    /// 上下文占用到窗口的这个比例时触发自动压缩；DSH 默认 0.8（compaction-basic: thresholdRatio）。
    pub compaction_threshold_ratio: f64,
    /// 压缩后保留的近期上下文比例，必须小于阈值；DSH 默认 0.16（retainRatio）。
    pub compaction_retain_ratio: f64,
    /// 压缩阈值公式里的预留输出 token；DSH 默认 65536（headroomTokens）。
    pub compaction_headroom_tokens: u64,
    /// 单个工具结果内联进上下文的 token 预算；0 表示禁用 spill（DSH 省略 maxInlineTokens 即禁用）。
    /// DSH 默认组合是 12500（bundle/base 的 spill-policy: maxInlineTokens）。
    pub spill_max_inline_tokens: usize,
    /// 用户级指令文件根目录；Agent 还会固定加入 <data-dir>。默认 ~/.agents（DSH 的 agents home）。
    pub instruction_roots: Vec<PathBuf>,
    /// 用户级 skill 根目录；Agent 还会固定加入 <data-dir>/skills。默认 ~/.agents/skills。
    pub skill_roots: Vec<PathBuf>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        // 没有 HOME（或不是绝对路径）时用户级根为空：不能把相对路径交给 validate。
        let home = home_dir().map(|home| home.join(".agents"));
        Self {
            max_parallel_tool_calls: 10,
            compaction_threshold_ratio: 0.8,
            compaction_retain_ratio: 0.16,
            compaction_headroom_tokens: 65_536,
            spill_max_inline_tokens: 12_500,
            skill_roots: home
                .as_ref()
                .map(|home| vec![home.join("skills")])
                .unwrap_or_default(),
            instruction_roots: home.map(|home| vec![home]).unwrap_or_default(),
        }
    }
}

impl AgentConfig {
    /// 建 agent 时校验一次：非法值返回 AgentError::InvalidConfig，不等到驱动任务里 panic。
    pub(crate) fn validate(&self) -> Result<(), AgentError> {
        if self.max_parallel_tool_calls == 0 {
            return Err(AgentError::InvalidConfig {
                field: "max_parallel_tool_calls",
                message: "必须是正整数".to_owned(),
            });
        }
        if !self.compaction_threshold_ratio.is_finite()
            || self.compaction_threshold_ratio <= 0.0
            || self.compaction_threshold_ratio > 1.0
        {
            return Err(AgentError::InvalidConfig {
                field: "compaction_threshold_ratio",
                message: "必须在 (0, 1] 之间".to_owned(),
            });
        }
        if !self.compaction_retain_ratio.is_finite()
            || self.compaction_retain_ratio < 0.0
            || self.compaction_retain_ratio >= self.compaction_threshold_ratio
        {
            return Err(AgentError::InvalidConfig {
                field: "compaction_retain_ratio",
                message: "必须 >= 0 且小于 compaction_threshold_ratio".to_owned(),
            });
        }
        if self.compaction_headroom_tokens == 0 {
            return Err(AgentError::InvalidConfig {
                field: "compaction_headroom_tokens",
                message: "必须是正整数".to_owned(),
            });
        }
        validate_roots("instruction_roots", &self.instruction_roots)?;
        validate_roots("skill_roots", &self.skill_roots)?;
        Ok(())
    }
}

/// 用户级根目录必须是绝对路径：相对路径会随进程 cwd 漂移。
fn validate_roots(field: &'static str, roots: &[PathBuf]) -> Result<(), AgentError> {
    for root in roots {
        if !root.is_absolute() || root.as_os_str().is_empty() {
            return Err(AgentError::InvalidConfig {
                field,
                message: "根目录必须是绝对路径".to_owned(),
            });
        }
    }
    Ok(())
}

/// 用户主目录；没有 HOME 或它不是绝对路径时返回 None（默认配置退化为空根）。
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| home.is_absolute())
}

/// 一轮的结束结果（订阅者看到的形式）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnOutcome {
    Completed,
    MaxTokens,
    Aborted,
    Error { code: String, message: String },
}

impl From<&TurnEndReason> for TurnOutcome {
    fn from(reason: &TurnEndReason) -> Self {
        match reason {
            TurnEndReason::Completed => Self::Completed,
            TurnEndReason::MaxTokens => Self::MaxTokens,
            TurnEndReason::Aborted => Self::Aborted,
            TurnEndReason::Error { code, message } => Self::Error {
                code: code.clone(),
                message: message.clone(),
            },
        }
    }
}

/// ask_user_question 的一条可选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuestionOption {
    pub label: String,
    pub description: Option<String>,
}

/// 需要用户回答的一条问题；id 与 answer 入口的 question_id 绑定。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Question {
    pub id: String,
    pub question: String,
    pub header: Option<String>,
    pub options: Vec<QuestionOption>,
    pub multi_select: bool,
}

/// 压缩的触发方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompactionTrigger {
    /// 越过阈值/上下文溢出触发的自动压缩。
    Auto,
    /// chat 的 /compact 或 compact() 入口触发。
    Manual,
}

/// 后台任务的结束状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobStatus {
    Completed,
    Failed,
    Killed,
}

/// 订阅者收到的 agent 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// 一段模型流出的文本（CLI 据此显示）。
    AssistantTextDelta { text: String },
    /// 一次请求失败后准备重试（CLI 显示「正在重试」；重试不写日志）。
    Retrying {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        code: String,
    },
    /// 一次回答的用量（厂商没给的项是 None，显示「未知」）。
    UsageReported { usage: Usage },
    /// 模型需要用户回答一组问题（T005.10 引入，T005.12 起携带全部问题；CLI 逐个问）。
    QuestionAsked { questions: Vec<Question> },
    /// 一次压缩开始（T005.8a/T005.8b 发）。
    CompactionStarted {
        compaction_id: String,
        trigger: CompactionTrigger,
    },
    /// 一次压缩结束；error 为 Some 表示失败。
    CompactionEnded {
        compaction_id: String,
        error: Option<String>,
    },
    /// 一个后台任务完成（T005.4 发；完成通知另外以下一步输入送达）。
    JobCompleted {
        job_id: String,
        status: JobStatus,
        detail: Option<String>,
    },
    /// 一轮结束；idle() 返回之前一定已经发出。
    TurnEnded { turn: u64, outcome: TurnOutcome },
}

/// agent 运行的依赖，都是具体类型（策略稿 5.A）。
pub struct Deps {
    pub model: DeepSeek,
    pub tools: ToolSet,
    pub store: SessionStore,
}

/// T004 的 agent：create/resume/send/cancel/subscribe/idle 是唯一入口。
#[derive(Clone)]
pub struct Agent {
    inner: Arc<driver::Inner>,
}

impl Agent {
    /// 新会话上启动 agent（会话日志由 store 建好，尚无事件）。
    pub async fn create(config: AgentConfig, deps: Deps) -> Result<Self, AgentError> {
        Ok(Self {
            inner: driver::Inner::start(config, deps, 0)?,
        })
    }

    /// 在已有会话上报文续接；从日志里最后一个 turn 编号之后继续。
    pub async fn resume(
        id: SessionId,
        config: AgentConfig,
        mut deps: Deps,
    ) -> Result<Self, AgentError> {
        if deps.store.id() != &id {
            return Err(AgentError::ResumeIdMismatch {
                requested: id.as_str().to_owned(),
                store: deps.store.id().as_str().to_owned(),
            });
        }
        // 续接先修复：给最后一个没收尾的 turn 补收尾（照 DSH 的 core/session/repair.ts）。
        // 只读日志扫描 + 一次批 append（seq 连续、fsync、失败回滚由 store 负责）；不重跑工具。
        let closers = session::interrupted_turn_closers(deps.store.events());
        if !closers.is_empty() {
            deps.store.append(&closers)?;
        }
        let last_turn = deps
            .store
            .events()
            .iter()
            .filter_map(|event| match &event.kind {
                session::EventKind::TurnStart { turn } => Some(*turn),
                _ => None,
            })
            .max()
            .unwrap_or(0);
        Ok(Self {
            inner: driver::Inner::start(config, deps, last_turn)?,
        })
    }

    /// 投递一条用户消息。delivery 决定它进入下一轮还是下一步。
    pub fn send(&self, message: UserMessage, delivery: Delivery) {
        self.inner.send(message, delivery);
    }

    /// 取消：清空收件箱；正在跑的一轮在下一次检查点停下。
    pub fn cancel(&self) {
        self.inner.cancel();
    }

    /// T005.10：回答问题。question_id 必须是当前等待的问题；返回是否送达。
    pub async fn answer(&self, question_id: &str, answer: &str) -> bool {
        self.inner.answer(question_id, answer)
    }

    /// 订阅 agent 事件。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.inner.subscribe()
    }

    /// 等到 agent 空闲（没有正在跑的轮次，也没有会唤醒它的排队输入）。
    pub async fn idle(&self) {
        self.inner.idle().await;
    }

    /// 手动压缩（chat 的 /compact）：空闲时立刻执行，运行中排到当前轮之后。
    pub async fn compact(&self) {
        let receiver = self.inner.queue_compaction();
        let _ = receiver.await;
    }

    pub fn id(&self) -> &SessionId {
        self.inner.id()
    }

    pub fn log_path(&self) -> &Path {
        self.inner.log_path()
    }
}
