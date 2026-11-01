//! 工具：Tool 接口、内置工具（read / glob / grep）和批执行。
//!
//! 机制照 DSH 的 packages/core/tools 与 packages/fs/tool-fs、packages/fs/tool-fs-search
//! （strategy 第 1 节）。

mod batch;
mod builtin;
mod fs;
mod session;

use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use model::ToolSchema;
use serde_json::Value;

pub use batch::{BatchOutcome, execute_batch};
pub use builtin::{ApplyPatchTool, EditTool, GlobTool, GrepTool, ReadTool, WriteTool};
pub use session::{
    FileVersion, ObservationStore, ObservedFile, SharedToolSession, SpillStore, TaskRegistry,
    ToolSessionState,
};

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
    /// 独立工具测试用：空观察表、空任务表、禁用 spill。
    pub fn new(cwd: impl Into<PathBuf>) -> Self {
        Self {
            cwd: cwd.into(),
            session: Arc::new(ToolSessionState::detached()),
        }
    }

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
}

impl ToolResult {
    pub fn success(call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            call_id: call_id.into(),
            content: content.into(),
            is_error: false,
            error_code: None,
        }
    }

    pub fn error(call_id: impl Into<String>, failure: ToolFailure) -> Self {
        Self {
            call_id: call_id.into(),
            content: format!("Error: {}", failure.message),
            is_error: true,
            error_code: failure.code,
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

    /// 并发类别；默认独占（保守，进屏障）。
    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Exclusive
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

    /// 开发用的内置工具集：read、glob、grep，和写类 write、edit、apply_patch。
    pub fn builtin() -> Self {
        Self::new(vec![
            Box::new(ReadTool::new()),
            Box::new(GlobTool::new()),
            Box::new(GrepTool::new()),
            Box::new(WriteTool::new()),
            Box::new(EditTool::new()),
            Box::new(ApplyPatchTool::new()),
        ])
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
