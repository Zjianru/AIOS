//! agent loop：一轮多步的驱动、收件箱，和 Agent 的 6 个入口。
//!
//! 机制照 DSH 的 packages/core/agent-loop（strategy 第 1 节）。

mod action_cards;
mod ask_capability;
mod assembly;
mod capabilities;
mod compaction;
mod compaction_capability;
mod driver;
mod error;
mod inbox;
mod instructions;
mod instructions_capability;
mod jobs_capability;
mod prune_capability;
mod reminder_capability;
mod skill_capability;
mod skill_catalog;
mod spill_capability;
mod todo_capability;
mod tool_gate;
mod tool_policy;
mod warn;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use model::{DeepSeek, Message, Usage, UserMessage};
use session::{SessionId, SessionStore, TurnEndReason};
use tools::ToolSet;

pub use assembly::{AgentOpenOptions, ToolPolicy};
pub use error::{AgentError, ManualCompactionErrorCode};
pub use inbox::Delivery;
pub use instructions::{MAX_CHAIN_BYTES, MAX_SOURCE_BYTES};
pub use tool_gate::loaded_skills;
pub use tool_policy::{DEFAULT_WAIT, NO_WAIT, STOP_AFTER_REPORT};
pub use warn::{WarningHook, set_warning_hook};

/// 按 provider/model 覆盖压缩策略（DSH compaction-basic 的 modelPolicies；只覆盖显式给出的字段）。
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionModelPolicy {
    pub provider: String,
    pub model: String,
    pub threshold_ratio: Option<f64>,
    pub retain_ratio: Option<f64>,
    pub headroom_tokens: Option<u64>,
    /// 摘要输出上限（DSH policy maxTokens）。
    pub summary_max_tokens: Option<u32>,
    /// 仍超阈值时最多再压几次（DSH compactionRetries）。
    pub compaction_retries: Option<u32>,
    /// 上下文溢出恢复最多重试几次（DSH maxOverflowRetries）。
    pub max_overflow_retries: Option<u32>,
}

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
    /// 摘要输出的 token 上限；None 时照 DSH 取 headroomTokens。
    pub compaction_summary_max_tokens: Option<u32>,
    /// 首次压缩后仍超阈值时最多再压几次；DSH 默认 1（compactionRetries）。
    pub compaction_retries: u32,
    /// 上下文溢出恢复最多重试几次；DSH 默认 1（maxOverflowRetries）。
    pub compaction_max_overflow_retries: u32,
    /// 按 provider/model 的压缩策略覆盖表（DSH modelPolicies；重复 target 非法）。
    pub compaction_model_policies: Vec<CompactionModelPolicy>,
    /// 单个工具结果内联进上下文的 token 预算；0 表示禁用 spill（DSH 省略 maxInlineTokens 即禁用）。
    /// DSH 默认组合是 12500（bundle/base 的 spill-policy: maxInlineTokens）。
    pub spill_max_inline_tokens: usize,
    /// 启动时清理多少天前的 spill 文件（照 DSH spill-local 的 cleanupPeriodDays；默认 30，0 = 不清理）。
    pub spill_cleanup_period_days: u64,
    /// 用户级指令文件根目录；Agent 还会固定加入 <data-dir>。默认 ~/.agents（DSH 的 agents home）。
    pub instruction_roots: Vec<PathBuf>,
    /// 用户级 skill 根目录；Agent 还会固定加入 <data-dir>/skills。默认 ~/.agents/skills。
    pub skill_roots: Vec<PathBuf>,
    /// 程序自带的 skill（正文编在二进制里，排在用户根后面）。
    ///
    /// 默认空：自带什么由产品装配决定（`Agent::open` 填入 `tools::bundled_skills()`）；
    /// 低层构造不替调用方猜产品资产。
    pub bundled_skills: Vec<tools::BundledSkill>,
    /// 宿主现在已存的工具凭证（自带 skill 与工具的条件每步求值一次）。
    ///
    /// 默认空：低层构造不假设任何凭证；产品的服务装配在打开根会话时接上 Keyring。
    pub tool_keys: tools::ToolKeySource,
    /// todo_write 是否允许多个 in_progress（照 DSH tool-todo 的 allowParallelInProgress；
    /// DSH bundle/base 设为 true）。false 时工具描述改成「只留一个」，多于一个 in_progress 的
    /// 调用被拒。Agent 在 create/resume 时用它替换内置 todo_write。
    pub allow_parallel_in_progress: bool,
    /// 是否拒绝会等待的工具调用（照 DSH noWait）：命中的调用先记 tool/call，再回错误
    /// tool/result，不执行。默认 false（CLI、现有测试不变）；Agent::open 从
    /// `tool_policy.no_wait` 填入。
    pub no_wait: bool,
    /// 追加在系统提示末尾的产品规则（照 DSH systemPrompt.section）；None = 不追加。
    /// 只在会话第一次请求建系统提示时生效；续接沿用日志里的系统提示。
    pub policy_prompt: Option<String>,
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
            compaction_summary_max_tokens: None,
            compaction_retries: 1,
            compaction_max_overflow_retries: 1,
            compaction_model_policies: Vec::new(),
            spill_max_inline_tokens: 12_500,
            spill_cleanup_period_days: 30,
            skill_roots: home
                .as_ref()
                .map(|home| vec![home.join("skills")])
                .unwrap_or_default(),
            bundled_skills: Vec::new(),
            tool_keys: tools::ToolKeySource::none(),
            instruction_roots: home.map(|home| vec![home]).unwrap_or_default(),
            allow_parallel_in_progress: true,
            no_wait: false,
            policy_prompt: None,
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
        if let Some(max_tokens) = self.compaction_summary_max_tokens
            && max_tokens == 0
        {
            return Err(AgentError::InvalidConfig {
                field: "compaction_summary_max_tokens",
                message: "必须是正整数".to_owned(),
            });
        }
        for (index, policy) in self.compaction_model_policies.iter().enumerate() {
            if policy.provider.trim().is_empty() || policy.model.trim().is_empty() {
                return Err(AgentError::InvalidConfig {
                    field: "compaction_model_policies",
                    message: format!("第 {index} 项 provider/model 不能为空"),
                });
            }
            if self.compaction_model_policies[..index]
                .iter()
                .any(|existing| {
                    existing.provider == policy.provider && existing.model == policy.model
                })
            {
                return Err(AgentError::InvalidConfig {
                    field: "compaction_model_policies",
                    message: format!(
                        "重复的 provider/model：{}/{}",
                        policy.provider, policy.model
                    ),
                });
            }
            if let Some(ratio) = policy.threshold_ratio
                && (!ratio.is_finite() || ratio <= 0.0 || ratio > 1.0)
            {
                return Err(AgentError::InvalidConfig {
                    field: "compaction_model_policies",
                    message: format!("第 {index} 项 threshold_ratio 必须在 (0, 1] 之间"),
                });
            }
            if let Some(max_tokens) = policy.summary_max_tokens
                && max_tokens == 0
            {
                return Err(AgentError::InvalidConfig {
                    field: "compaction_model_policies",
                    message: format!("第 {index} 项 summary_max_tokens 必须是正整数"),
                });
            }
            let threshold = policy
                .threshold_ratio
                .unwrap_or(self.compaction_threshold_ratio);
            let retain = policy.retain_ratio.unwrap_or(self.compaction_retain_ratio);
            if !retain.is_finite() || retain < 0.0 || retain >= threshold {
                return Err(AgentError::InvalidConfig {
                    field: "compaction_model_policies",
                    message: format!("第 {index} 项 retain_ratio 必须 >= 0 且小于 threshold_ratio"),
                });
            }
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
            TurnEndReason::Aborted { .. } => Self::Aborted,
            // 续接修复补的收尾不是一次实时跑完的轮次，订阅者事件里没有它。
            TurnEndReason::Interrupted => Self::Aborted,
            TurnEndReason::Error { error } => Self::Error {
                code: error.code.clone(),
                message: error.message.clone(),
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

/// 一次成功压缩的可观察结果（DSH command-compact 的 CompactionResult 计数部分）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CompactionOutcome {
    /// 被替换的 surface 节点数（DSH shadowedSeqs.length）。
    pub shadowed_items: usize,
    /// 被替换内容的估算 token 数（DSH shadowedTokenCount）。
    pub shadowed_tokens: u64,
}

/// 订阅者收到的 agent 事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentEvent {
    /// 一段模型流出的文本（CLI 据此显示）。
    AssistantTextDelta { text: String },
    /// 工具调用参数的增量（照 DSH tool-call-delta）：只给订阅者，不落日志。
    /// 同一 callId 的后续帧沿用首帧的 name（模型流里的 name 只在起始帧给）。
    ToolCallDelta {
        call_id: String,
        name: String,
        arguments_delta: String,
    },
    /// 一次请求失败后准备重试（CLI 显示「正在重试」；重试不写日志）。
    Retrying {
        attempt: u32,
        max_attempts: u32,
        delay_ms: u64,
        code: String,
    },
    /// 一次回答的用量（厂商没给的项是 None，显示「未知」）。
    UsageReported { usage: Usage },
    /// 模型需要用户回答一组问题（引入，起携带全部问题；CLI 逐个问）。
    QuestionAsked { questions: Vec<Question> },
    /// 一次压缩开始（发）。
    CompactionStarted {
        compaction_id: String,
        trigger: CompactionTrigger,
    },
    /// 一次压缩结束；error 为 Some 表示失败。
    CompactionEnded {
        compaction_id: String,
        error: Option<String>,
    },
    /// 一个后台任务完成（发；完成通知另外以下一步输入送达）。
    JobCompleted {
        job_id: String,
        status: JobStatus,
        detail: Option<String>,
    },
    /// 输入队列：一条消息被插进待处理列表（DSH agent/inbox/inserted；只是通知，不落日志）。
    InboxInserted { message: Message },
    /// 输入队列：一条消息被领进某一步（DSH agent/inbox/claimed；只是通知，不落日志）。
    InboxClaimed { message: Message, turn: u64 },
    /// 输入队列：一条待处理消息被取消丢掉（DSH agent/inbox/discarded；只是通知，不落日志）。
    InboxDiscarded { message: Message },
    /// 一轮结束；idle() 返回之前一定已经发出。
    TurnEnded { turn: u64, outcome: TurnOutcome },
}

/// agent 运行的依赖，都是具体类型（策略稿 5.A）。
pub struct Deps {
    pub model: DeepSeek,
    pub tools: ToolSet,
    pub store: SessionStore,
}

/// Agent 启动时照 DSH spill-local 的 cleanupPeriodDays 一次性清理过期 spill 文件。
/// best-effort（DSH 也是启动时异步 sweep、失败只 warn）：不阻塞/不打断会话启动。
fn cleanup_spill_files(store: &SessionStore, period_days: u64) {
    if period_days == 0 {
        return;
    }
    let Some(data_dir) = store.path().parent().and_then(std::path::Path::parent) else {
        return;
    };
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(
            period_days.saturating_mul(86_400),
        ))
        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
    let _ = tools::spill::sweep_spill_root(&data_dir.join("spill"), cutoff);
}

/// 带 requestId 的发送受理结果（durable rpcId 幂等）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendAdmission {
    /// 新消息已进收件箱并唤醒（或排队）。
    Admitted,
    /// 同一个 requestId 已经受理过（inbox 或日志里已有），没有重复插入。
    Duplicate,
}

/// 的 agent。产品组装入口是 Agent::open（crates/agent/src/assembly.rs）；
/// create/resume 是 deps 注入的低层构造，留给测试/定制工具集或已有 Store 的调用方。
#[derive(Clone)]
pub struct Agent {
    inner: Arc<driver::Inner>,
}

impl Agent {
    /// 选模型（下一轮请求生效；正在跑的请求不受影响，D26）。
    pub fn select_model(&self, model: String) {
        self.inner.model.set_model(model);
    }

    /// 换模型的上下文窗口（下一轮生效）：压缩阈值按它算，不按装配时的旧模型。
    pub fn set_context_window(&self, window: u64) {
        self.inner.model.set_context_window(window);
    }

    /// 记一条确认卡决定（用户点了按钮之后的事实）。决定落日志，下一次请求前由
    /// action-cards 能力通知模型；重开页面从同一条日志恢复回执。
    pub async fn record_card_decision(
        &self,
        decision: &session::CardDecision,
    ) -> Result<(), AgentError> {
        self.inner.append(&[decision.event()]).await
    }

    /// 用户选择连接/模型/档（下一轮生效，D26）。选择是 durable 事实：写
    /// session/model 事件（重开会话从日志恢复）；本轮已冻结的 model/effort 不受影响。
    pub async fn select(&self, selection: &session::ModelSelection) -> Result<(), AgentError> {
        self.inner
            .append(&[session::EventKind::SessionModel {
                connection: selection.connection.clone(),
                model: selection.model.clone(),
                effort_wanted: selection.effort_wanted.clone(),
            }])
            .await?;
        self.inner.model.set_model(selection.model.clone());
        self.inner
            .model
            .set_effort(model::Effort::new(selection.effort_wanted.clone()));
        Ok(())
    }

    /// 新会话上启动 agent（会话日志由 store 建好，尚无事件）。
    ///
    /// 配置非法时在建之前返回 Err（不清理 spill、不动工具集、不留下驱动任务）。
    pub async fn create(config: AgentConfig, mut deps: Deps) -> Result<Self, AgentError> {
        config.validate()?;
        cleanup_spill_files(&deps.store, config.spill_cleanup_period_days);
        deps.tools
            .set_todo_parallel(config.allow_parallel_in_progress);
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
        // 配置校验先于任何副作用（spill 清理/修复 append）。
        config.validate()?;
        cleanup_spill_files(&deps.store, config.spill_cleanup_period_days);
        deps.tools
            .set_todo_parallel(config.allow_parallel_in_progress);
        // 续接先修复：给最后一个没收尾的 turn 补收尾（照 DSH 的 core/session/repair.ts）。
        // 只读日志扫描 + 一次批 append（seq 连续、fsync、失败回滚由 store 负责）；不重跑工具。
        if let Some(closers) = session::interrupted_turn_closers(deps.store.events()) {
            deps.store.append_with_time(&closers.events, closers.time)?;
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
    /// 返回时消息已经写进会话日志（重启后队列还在）。
    pub async fn send(&self, message: UserMessage, delivery: Delivery) -> Result<(), AgentError> {
        self.inner.send(message, delivery).await
    }

    /// 带调用方 requestId 的发送（DSH prompt 的 requestId；durable 幂等）。
    /// 同一个 id 已进 inbox 或已落日志时返回 Duplicate，不再插入、不再唤醒。
    pub async fn send_with_request_id(
        &self,
        request_id: &str,
        message: UserMessage,
        delivery: Delivery,
    ) -> Result<SendAdmission, AgentError> {
        self.inner
            .send_with_request_id(request_id, message, delivery)
            .await
    }

    /// 重启后领取 pending inbox（有排队输入就开一轮）；返回是否领取了东西。
    /// 只由重启路径显式调用（2a 的 resume 只恢复、不唤醒）。
    pub fn claim_pending(&self) -> bool {
        self.inner.claim_pending()
    }

    /// follow/events 的日志读面（seq > after_seq，按日志顺序）。
    pub fn session_events_from(&self, after_seq: u64) -> Vec<session::Event> {
        self.inner.session_events_from(after_seq)
    }

    /// 日志最后一条事件的 seq（空日志为 0）。
    pub fn last_seq(&self) -> u64 {
        self.inner.last_seq()
    }

    /// 取消：清空收件箱并中止正在跑的一轮；清空也写进会话日志。
    pub async fn cancel(&self) -> Result<(), AgentError> {
        self.inner.cancel().await
    }

    /// 取消当前一轮，但保留输入队列：队列里的消息还在，下一轮照常领取。
    ///
    /// `cause` 写进 turn/end 的 aborted 原因（例如内核钩子 model-disabled）。
    pub fn cancel_with(&self, cause: session::AbortCause) {
        self.inner.cancel_with(cause);
    }

    /// 收尾停：给这个会话打收尾标记，下一次工具调用只放行 task_report。
    ///
    /// 标记只活这个 Agent 实例（一次 Attempt）；下一次 Attempt 开的是新会话，不带标记。
    pub fn stop_after_report(&self) {
        // 骨架：统一工具入口的拒绝规则还没接上，标记暂不生效。
    }

    /// 本轮冻结的模型（适配器在 begin_turn 时确定）；没有正在跑的一轮时为 None。
    pub fn frozen_model(&self) -> Option<String> {
        self.inner.model.frozen_model()
    }

    /// 回答问题。question_id 必须是当前等待的问题；返回是否送达。
    /// 起答案照 DSH 形状：selected 是选中的选项标签（可多个），custom 是自由文本
    /// （K14；DSH `tool-ask-user/src/index.ts:111-116`）。
    pub async fn answer(
        &self,
        question_id: &str,
        selected: Vec<String>,
        custom: Option<String>,
    ) -> bool {
        self.inner.answer(question_id, selected, custom).await
    }

    /// 撤销一条待答问题（提问服务路径）。等待中的问题让这一批以 DSH 的
    /// ASK_ABORTED 工具错误收尾、本轮继续；重启恢复的问题写 question/cancelled 后移除。
    /// 返回是否确实撤销了待答问题。
    pub async fn cancel_question(&self, question_id: &str) -> bool {
        self.inner.cancel_question(question_id).await
    }

    /// 当前待答问题（等待中 + 重启恢复）；界面服务 从这里接。
    pub fn pending_questions(&self) -> Vec<Question> {
        self.inner.pending_questions()
    }

    /// 订阅 agent 事件。
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.inner.subscribe()
    }

    /// 日志事件的 live 订阅（服务事件面）：每条带日志 seq/type/data，
    /// 由服务宿主转发给载体；容量兜底用 Lagged（重新跟随在）。
    pub fn subscribe_session_events(&self) -> tokio::sync::broadcast::Receiver<session::Event> {
        self.inner.subscribe_session_events()
    }

    /// 被动状态用的运行态（有一轮在跑）。
    pub fn is_running(&self) -> bool {
        self.inner.is_running()
    }

    /// 等到 agent 空闲（没有正在跑的轮次，也没有会唤醒它的排队输入）。
    pub async fn idle(&self) {
        self.inner.idle().await;
    }

    /// 手动压缩（chat 的 /compact，照 DSH command-compact）：空闲时立刻执行，运行中排到
    /// 当前轮之后；`source_command_id` 会写进 compaction/* 事件。返回压缩结果计数，
    /// 没有可压缩历史时返回 None。
    pub async fn compact(
        &self,
        source_command_id: Option<String>,
    ) -> Result<Option<CompactionOutcome>, AgentError> {
        self.inner.compact_manual(source_command_id).await
    }

    pub fn id(&self) -> &SessionId {
        self.inner.id()
    }

    pub fn log_path(&self) -> &Path {
        self.inner.log_path()
    }

    /// 会话的工作目录（规范化后的；工具的路径相对它解析，页面用它显示相对路径）。
    pub fn cwd(&self) -> &Path {
        self.inner.cwd()
    }
}
