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

use std::collections::BTreeSet;
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
    BundledSkill, SKILL_FILE, SkillContent, SkillDiscovery, SkillFrontmatter, SkillInvocation,
    SkillSummary, SkillWarning, bundled_skills, catalog_description, discover_skill_catalog,
    discover_skills, invoked_skill_names, is_skill_name, load_skill, load_user_invocable_skill,
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

/// 会话日志的读取面：工具在调用时才能看到当前会话的事实（用量等）。
///
/// 用回调而不是快照，是因为每步都拷一份整条日志太贵；只有真要查的工具才会读。
/// 本 crate 里 `session` 是工具会话状态模块，会话日志的 crate 要写全路径 `::session`。
pub type SessionEventsReader = Arc<dyn Fn() -> Vec<::session::Event> + Send + Sync>;

/// 一次工具调用的执行上下文。
#[derive(Clone)]
pub struct ToolContext {
    /// 会话的工作目录；相对路径都相对它解析。
    pub cwd: PathBuf,
    /// 会话级工具状态（观察表 / 任务注册表 / spill）；由 Agent 建、跨调用共享。
    session: Arc<ToolSessionState>,
    /// 本步按条件开出的工具名；None = 不按条件筛（只驱动单个工具的调用方）。
    enabled: Option<Arc<std::collections::BTreeSet<String>>>,
    /// 会话日志的读取面；没有会话日志时返回空。
    session_events: SessionEventsReader,
}

impl std::fmt::Debug for ToolContext {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ToolContext")
            .field("cwd", &self.cwd)
            .field("enabled", &self.enabled)
            .finish_non_exhaustive()
    }
}

impl ToolContext {
    /// Agent 的批执行用：带会话级状态。
    pub fn with_session(cwd: impl Into<PathBuf>, session: Arc<ToolSessionState>) -> Self {
        Self {
            cwd: cwd.into(),
            session,
            enabled: None,
            session_events: Arc::new(Vec::new),
        }
    }

    /// 本步开出的工具名（内核每步给一次）；没给就按无门处理。
    pub fn gated(mut self, enabled: Arc<std::collections::BTreeSet<String>>) -> Self {
        self.enabled = Some(enabled);
        self
    }

    /// 接上会话日志的读取面（内核给的是会话日志的内存视图）。
    pub fn observing_session(mut self, events: SessionEventsReader) -> Self {
        self.session_events = events;
        self
    }

    /// 这个工具名在本步能不能执行（不满足条件的不进表，也不执行）。
    pub fn allows(&self, name: &str) -> bool {
        self.enabled
            .as_ref()
            .is_none_or(|enabled| enabled.contains(name))
    }

    /// 当前会话的日志快照（工具调用时现读）。
    pub fn session_events(&self) -> Vec<::session::Event> {
        (self.session_events)()
    }

    pub fn session(&self) -> &ToolSessionState {
        &self.session
    }

    pub fn shared_session(&self) -> Arc<ToolSessionState> {
        Arc::clone(&self.session)
    }
}

/// 工具凭证的种类（D73）：只有 Exa 的 key（联网搜索用它）。
///
/// 程序自带的 skill 和工具都可以挂在一种凭证上：凭证没存时，skill 不进目录、
/// 工具不开出。凭证的事实由产品装配提供（`ToolKeySource`），内核不自己读 Keyring。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolKey {
    Exa,
}

/// 当前已存的工具凭证：产品装配时注入的求值面，每步查一次。
///
/// 默认空（内核测试、CLI 不假设任何工具凭证）。读不出来时按「没存」处理：
/// 设置页另外报读不出的原因，不在这里重复报。
#[derive(Clone, Default)]
pub struct ToolKeySource(Option<Arc<dyn Fn() -> BTreeSet<ToolKey> + Send + Sync>>);

impl ToolKeySource {
    /// 什么凭证都没有。
    pub const fn none() -> Self {
        Self(None)
    }

    /// 产品装配：每次求值返回现在已存的凭证。
    pub fn new(source: Arc<dyn Fn() -> BTreeSet<ToolKey> + Send + Sync>) -> Self {
        Self(Some(source))
    }

    /// 现在已存的凭证。
    pub fn present(&self) -> BTreeSet<ToolKey> {
        self.0
            .as_ref()
            .map_or_else(BTreeSet::new, |source| source())
    }
}

impl std::fmt::Debug for ToolKeySource {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let state = if self.0.is_some() { "<source>" } else { "none" };
        formatter
            .debug_tuple("ToolKeySource")
            .field(&state)
            .finish()
    }
}

/// 只有同一份来源的两个副本相等；空来源与空来源相等（`AgentConfig` 的比较用）。
impl PartialEq for ToolKeySource {
    fn eq(&self, other: &Self) -> bool {
        match (&self.0, &other.0) {
            (None, None) => true,
            (Some(left), Some(right)) => Arc::ptr_eq(left, right),
            _ => false,
        }
    }
}

/// 工具的开出条件：每步开始时算一次，不满足的不进本步的工具表，也不能执行。
///
/// 照 DSH core/tools 的 `when`（`core-tools-contracts.ts:123`、action-cards `:211`）：
/// 条件挂在工具注册处，由宿主每步求值一次；会变的条件有两类——模型在本会话里加载过
/// 哪个 skill，以及宿主有没有存对应的工具凭证。skill 名字是编进二进制的那几个，
/// 所以用静态切片，不另造类型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolCondition {
    /// 总是开出（内置工具）。
    Always,
    /// 本会话里 `skill` 工具成功加载过这些名字之一以后开出。名单由登记表算出来
    /// （服务装配时定），所以是拥有的字符串而不是静态切片。
    AfterSkill(Vec<String>),
    /// 加载过这些 skill 之一，并且已经存了这种工具凭证。
    AfterSkillWithKey { skills: Vec<String>, key: ToolKey },
}

impl ToolCondition {
    /// 条件在这一次求值里是否满足；`loaded` 是已成功加载的 skill 名，
    /// `keys` 是宿主现在已存的工具凭证。
    pub fn satisfied_by(&self, loaded: &BTreeSet<String>, keys: &BTreeSet<ToolKey>) -> bool {
        match self {
            Self::Always => true,
            Self::AfterSkill(names) => names.iter().any(|name| loaded.contains(name)),
            Self::AfterSkillWithKey { skills, key } => {
                keys.contains(key) && skills.iter().any(|name| loaded.contains(name))
            }
        }
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

    /// 开出条件；默认总是开出。挂了条件的工具由宿主每步求值一次。
    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    /// 执行一次调用。实现方不 panic；失败用 is_error 表达。
    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult>;
}

/// 一次请求可用的工具集合。
///
/// 内部持有 `Arc`：产品装配时要先造好工具再交给 Agent（`AgentOpenOptions` 可克隆）。
#[derive(Default)]
pub struct ToolSet {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolSet {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn new(tools: Vec<Box<dyn Tool>>) -> Self {
        Self {
            tools: tools.into_iter().map(Arc::from).collect(),
        }
    }

    /// 加一个装配层已经造好的工具（产品的额外工具，例如服务的查询工具）。
    pub fn push_shared(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
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
                Arc::new(TodoWriteTool::new_with_parallel(allow_parallel_in_progress));
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
            .map(|tool| schema_of(tool.as_ref()))
            .collect()
    }

    /// 本步开出的工具名：每个工具的条件求值一次（照 DSH 的 `when`）。
    pub fn enabled(
        &self,
        loaded_skills: &BTreeSet<String>,
        tool_keys: &BTreeSet<ToolKey>,
    ) -> BTreeSet<String> {
        self.tools
            .iter()
            .filter(|tool| tool.condition().satisfied_by(loaded_skills, tool_keys))
            .map(|tool| tool.name().to_owned())
            .collect()
    }

    /// 本步的请求工具表：只含开出的工具，按注册顺序。
    pub fn definitions_for(&self, enabled: &BTreeSet<String>) -> Vec<ToolSchema> {
        self.tools
            .iter()
            .filter(|tool| enabled.contains(tool.name()))
            .map(|tool| schema_of(tool.as_ref()))
            .collect()
    }
}

/// 一个工具的模型可见定义。
fn schema_of(tool: &dyn Tool) -> ToolSchema {
    ToolSchema {
        name: tool.name().to_owned(),
        description: tool.description().to_owned(),
        parameters: tool.parameters().clone(),
        defer_loading: false,
    }
}
