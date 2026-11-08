//! `aios_query`：Kind 查 AIOS 现在的设置、连接、模型、用量和产品信息。
//!
//! 只读服务里**已经存下的**数据（D71）：设置读 `settings.json`、连接读 Keyring 的视图、
//! 模型读目录和存下的厂商列表、用量读会话日志、产品信息是编进二进制的常数。不现拉厂商
//! 列表、不现测连接；输出里没有 key 正文，也不带连接的地址和鉴权头。
//!
//! 挂在 skill `aios-settings` 后面：本会话加载过它以后，下一步才开出这个工具（T014.2）。

use std::path::PathBuf;

use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 查询的五个主题。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Topic {
    Settings,
    Connections,
    Models,
    Usage,
    Product,
}

impl Topic {
    fn parse(raw: &str) -> Option<Self> {
        match raw {
            "settings" => Some(Self::Settings),
            "connections" => Some(Self::Connections),
            "models" => Some(Self::Models),
            "usage" => Some(Self::Usage),
            "product" => Some(Self::Product),
            _ => None,
        }
    }
}

/// 只读查询工具。
pub struct QueryTool {
    data_dir: PathBuf,
    parameters: Value,
}

impl QueryTool {
    pub fn new(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            data_dir: data_dir.into(),
            parameters: json!({
                "type": "object",
                "properties": {
                    "topic": {
                        "type": "string",
                        "enum": ["settings", "connections", "models", "usage", "product"],
                        "description": "settings：这条对话的模型和推理强度、币种；connections：已存的连接和它们的验证状态；models：能选的模型和屏蔽表；usage：这条对话的轮数、token 和花费；product：AIOS 的版本和能力边界。"
                    }
                },
                "required": ["topic"]
            }),
        }
    }
}

impl Tool for QueryTool {
    fn name(&self) -> &str {
        "aios_query"
    }

    fn description(&self) -> &str {
        "Read AIOS's own current settings, connections, models, this conversation's usage, or what AIOS can do. Read-only: it never changes anything and never returns API keys."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkill(&["aios-settings"])
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let topic = match parse_topic(&call.arguments) {
                Ok(topic) => topic,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            ToolResult::error(
                call.id,
                ToolFailure::plain(format!("topic {:?} 还没实现", topic)),
            )
        })
    }
}

fn parse_topic(arguments: &str) -> Result<Topic, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let name = args
        .get("topic")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("invalid topic: expected a non-empty string"))?;
    Topic::parse(name).ok_or_else(|| {
        ToolFailure::plain(format!(
            "invalid topic \"{name}\": expected settings, connections, models, usage or product"
        ))
    })
}
