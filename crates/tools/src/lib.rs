//! 工具：Tool 接口、内置工具（read / glob / grep）和批执行。
//!
//! 机制照 DSH 的 packages/core/tools 与 packages/fs/tool-fs、packages/fs/tool-fs-search
//! （strategy 第 1 节）。

pub mod attachments;
mod batch;
mod builtin;
pub mod diff;
pub mod file_references;
mod fs;
mod jobs;
mod session;
mod skills;
pub mod spill;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use model::ToolSchema;
use serde_json::Value;

pub use batch::{BatchOutcome, execute_batch};
pub use builtin::{
    ApplyPatchTool, AskUserQuestionTool, BashTool, CanvasTool, EditTool, GlobTool, GrepTool,
    JobKillTool, JobListTool, JobOutputTool, ReadTool, SkillTool, TodoWriteTool, WriteTool,
};
pub use diff::{DIFF_CONTEXT, FileDiff, compute_hunk_diffs};
pub use jobs::{
    JobCause, JobNotifyCallback, JobOutputRead, JobRead, JobRegistered, JobRegisteredCallback,
    JobSettled, JobSettledCallback, JobSpec, JobStatus, JobToolCall, JobView, MAX_WAIT_MS,
    TaskRegistry, render_model_delta,
};
pub use session::{
    FileVersion, ObservationPolicy, ObservedFile, SharedToolSession, SpillStore, ToolSessionState,
    WriteIntent,
};
pub use skills::{
    SKILL_FILE, SkillContent, SkillDiscovery, SkillFrontmatter, SkillInvocation, SkillSummary,
    SkillWarning, catalog_description, discover_skill_catalog, discover_skills,
    invoked_skill_names, is_skill_name, load_skill, load_user_invocable_skill,
    parse_skill_frontmatter, render_skill_content, skill_roots,
};
pub use spill::{SpillRef, bound_result, estimate_text_tokens};

/// 工具调用的规范化参数：JSON 深度 key 排序后序列化，属性顺序不同也视为同一调用。
/// 重复调用提醒（T005.9）用它构造身份键；解析失败的原始字符串直接参与比较。
pub fn canonical_arguments(arguments: &str) -> String {
    fn sort(value: serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let sorted: std::collections::BTreeMap<String, serde_json::Value> = map
                    .into_iter()
                    .map(|(key, value)| (key, sort(value)))
                    .collect();
                serde_json::Value::Object(sorted.into_iter().collect())
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.into_iter().map(sort).collect())
            }
            other => other,
        }
    }
    match serde_json::from_str::<serde_json::Value>(arguments) {
        Ok(value) => serde_json::to_string(&sort(value)).unwrap_or_else(|_| arguments.to_owned()),
        Err(_) => arguments.to_owned(),
    }
}

/// 一个异步结果的 boxed future。Tool 必须能放进 dyn 的工具集，所以用 boxed
/// future，而不是返回位置的 impl Future（后者不能让 trait 成为 dyn 兼容）。
pub type ToolFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 一次工具调用的执行上下文。
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// 会话的工作目录；相对路径都相对它解析。
    pub cwd: PathBuf,
    /// 会话级工具状态（观察表 / 任务注册表 / spill）；由 Agent 建、跨调用共享。
    session: Arc<ToolSessionState>,
}

impl ToolContext {
    /// Agent 的批执行用：带会话级状态。
    pub fn with_session(cwd: impl Into<PathBuf>, session: Arc<ToolSessionState>) -> Self {
        Self {
            cwd: cwd.into(),
            session,
        }
    }

    pub fn session(&self) -> &ToolSessionState {
        &self.session
    }

    pub fn shared_session(&self) -> Arc<ToolSessionState> {
        Arc::clone(&self.session)
    }
}

/// 工具的并发类别：独占调用是屏障，并行安全的调用进有界滚动池。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolConcurrency {
    Parallel,
    Exclusive,
}

/// 一次工具调用的请求：arguments 是模型给出的原始 JSON 字符串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 工具失败：稳定错误码（可缺省）+ 可读消息（模型看到的是 Error: <message>）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolFailure {
    pub code: Option<String>,
    pub message: String,
}

impl ToolFailure {
    /// 普通工具参数错误：没有结构化错误码（照 DSH 的 ordinary tool argument error）。
    pub fn plain(message: impl Into<String>) -> Self {
        Self {
            code: None,
            message: message.into(),
        }
    }

    /// 带稳定错误码的失败（如 FS_NOT_FOUND、SEARCH_INVALID_PATTERN）。
    pub fn coded(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: Some(code.into()),
            message: message.into(),
        }
    }
}

/// 一次工具调用的结果，按模型给出的顺序回给模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
    pub error_code: Option<String>,
    /// T013.1：展示用元信息（diff 卡片等）；落进会话日志的 tool/result.meta。
    pub meta: Option<serde_json::Value>,
}

impl ToolResult {
    pub fn success(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: false,
            error_code: None,
            meta: None,
        }
    }

    /// 带展示元信息的成功结果（T013.1：write/edit/apply_patch 的 diff）。
    pub fn success_with_meta(
        call_id: impl Into<String>,
        content: impl Into<String>,
        meta: serde_json::Value,
    ) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: false,
            error_code: None,
            meta: Some(meta),
        }
    }

    pub fn error(call_id: impl Into<String>, failure: ToolFailure) -> Self {
        Self {
            call_id: call_id.into(),
            content: format!("Error: {}", failure.message),
            is_error: true,
            error_code: failure.code,
            meta: None,
        }
    }
}

/// 工具接口。实现方负责参数校验、上限和错误文案。
pub trait Tool: Send + Sync {
    /// 模型看到的工具名。
    fn name(&self) -> &str;

    /// 模型看到的工具描述。
    fn description(&self) -> &str;

    /// JSON Schema 形式的参数定义。
    fn parameters(&self) -> &Value;

    /// 工具声明自己的单次调用时间上限（毫秒）；None 表示不限时（照 DSH ToolDefinition.timeoutMs）。
    /// 默认 None：现有工具行为不变；批执行在超时后返回 TOOL_TIMEOUT 错误结果。
    fn timeout_ms(&self) -> Option<u64> {
        None
    }

    /// 并发类别；默认独占（保守，进屏障）。
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Exclusive
    }

    /// 是否为内置 todo_write（T006.12：Agent 按 AgentConfig.allow_parallel_in_progress
    /// 在装配时替换它的描述与校验策略；自定义同名工具默认 false，不被替换）。
    fn is_builtin_todo_write(&self) -> bool {
        false
    }

    /// 执行一次调用。实现方不 panic；失败用 is_error 表达。
    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult>;
}

/// 一次请求可用的工具集合。
#[derive(Default)]
pub struct ToolSet {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self { tools }
    }

    /// 开发用的内置工具集。T006.6 起默认注册照 DSH：read、glob、grep，写类 write、edit
    /// （apply_patch 不默认注册，DSH tool-fs 的 applyPatch 默认 false），bash，和后台任务三件。
    pub fn builtin() -> Self {
        Self::builtin_tools(false)
    }

    /// 显式开启 apply_patch 的内置工具集（等价 DSH tool-fs 配置 applyPatch: true）。
    pub fn builtin_with_patch() -> Self {
        Self::builtin_tools(true)
    }

    fn builtin_tools(include_patch: bool) -> Self {
        let mut tools: Vec<Box<dyn Tool>> = vec![
            Box::new(ReadTool::new()),
            Box::new(GlobTool::new()),
            Box::new(GrepTool::new()),
            Box::new(WriteTool::new()),
            Box::new(EditTool::new()),
        ];
        // T006.6：默认注册照 DSH tool-fs（applyPatch 默认 false，显式配置才注册）。
        if include_patch {
            tools.push(Box::new(ApplyPatchTool::new()));
        }
        tools.extend([
            Box::new(BashTool::new()) as Box<dyn Tool>,
            Box::new(JobOutputTool::new()),
            Box::new(JobListTool::new()),
            Box::new(JobKillTool::new()),
            Box::new(SkillTool::new()),
            Box::new(TodoWriteTool::new()),
            Box::new(AskUserQuestionTool::new()),
            // T015.1：画布（D58）；默认注册，Kind 自己决定什么时候画。
            Box::new(CanvasTool::new()),
        ]);
        Self::new(tools)
    }

    /// T006.12：按部署配置替换内置 todo_write 的并行策略（Agent 在 create/resume 时按
    /// AgentConfig.allow_parallel_in_progress 调；自定义 todo 工具不受影响）。
    pub fn set_todo_parallel(&mut self, allow_parallel_in_progress: bool) {
        if let Some(index) = self
            .tools
            .iter()
            .position(|tool| tool.is_builtin_todo_write())
        {
            self.tools[index] =
                Box::new(TodoWriteTool::new_with_parallel(allow_parallel_in_progress));
        }
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.name() == name)
            .map(AsRef::as_ref)
    }

    /// 模型可见的工具定义（请求体里的 tools 字段）。
    pub fn definitions(&self) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .map(|tool| ToolSchema {
                name: tool.name().to_owned(),
                description: tool.description().to_owned(),
                parameters: tool.parameters().clone(),
            })
            .collect()
    }
}
