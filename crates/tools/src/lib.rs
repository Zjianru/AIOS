//! 工具：Tool 接口和工具集。T004.1 只有接口；内置工具在 T004.2 加入。

use std::future::Future;
use std::pin::Pin;

use serde_json::Value;

/// 一个异步结果的 boxed future。Tool 必须能放进 dyn 的工具集，所以用
/// boxed future，而不是返回位置的 impl Future（后者不能让 trait 成为 dyn 兼容）。
pub type ToolFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// 一次工具调用的请求：arguments 是模型给出的原始 JSON 字符串。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// 一次工具调用的结果，按模型给出的顺序回给模型。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
}

/// 工具接口。实现方负责参数校验、上限和错误文案。
pub trait Tool: Send + Sync {
    /// 模型看到的工具名。
    fn name(&self) -> &str;

    /// 模型看到的工具描述。
    fn description(&self) -> &str;

    /// JSON Schema 形式的参数定义。
    fn parameters(&self) -> &Value;

    /// 执行一次调用。实现方不 panic；失败用 is_error 表达。
    fn execute(&self, call: ToolCall) -> ToolFuture<'_, ToolResult>;
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

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }

    pub fn tools(&self) -> &[Box<dyn Tool>] {
        &self.tools
    }
}
