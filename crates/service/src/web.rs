//! 网页工具：`web_search` 和 `web_fetch` 的模型面（名字、说明、参数、超时、开出条件）。
//!
//! 这一张票只把工具声明出来：加载过 `web` 且存了 Exa 的 key，下一步的工具表里才有它们。
//! 执行（Exa 搜索、带拦截的读网页）在接下来的票里接上。

use serde_json::{Value, json};
use tools::{
    Tool, ToolCall, ToolCondition, ToolContext, ToolFailure, ToolFuture, ToolKey, ToolResult,
};

/// 一次网页工具调用的时间上限（和 Exa 请求、读网页共用 30 秒）。
const WEB_TOOL_TIMEOUT_MS: u64 = 30_000;

/// 联网搜索工具。
pub struct WebSearchTool {
    parameters: Value,
}

impl WebSearchTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "queries": {
                        "type": "array",
                        "items": {"type": "string"},
                        "minItems": 1,
                        "maxItems": 4,
                        "description": "1–4 search queries; their results are merged."
                    }
                },
                "required": ["queries"]
            }),
        }
    }
}

impl Default for WebSearchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WebSearchTool {
    fn name(&self) -> &str {
        "web_search"
    }

    fn description(&self) -> &str {
        "Search the web for current information. Returns up to 8 sources with summaries and URLs; cite the URLs you use."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(WEB_TOOL_TIMEOUT_MS)
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkillWithKey {
            skills: vec!["web".to_owned()],
            key: ToolKey::Exa,
        }
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            ToolResult::error(
                call.id,
                ToolFailure::plain("web_search is not wired up yet; answer without it"),
            )
        })
    }
}

/// 读网页工具。
pub struct WebFetchTool {
    parameters: Value,
}

impl WebFetchTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "url": {
                        "type": "string",
                        "description": "The HTTP(S) URL to fetch."
                    }
                },
                "required": ["url"]
            }),
        }
    }
}

impl Default for WebFetchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WebFetchTool {
    fn name(&self) -> &str {
        "web_fetch"
    }

    fn description(&self) -> &str {
        "Fetch the content of a specific HTTP(S) URL and return it as text. HTML pages are converted to Markdown."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(WEB_TOOL_TIMEOUT_MS)
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::AfterSkillWithKey {
            skills: vec!["web".to_owned()],
            key: ToolKey::Exa,
        }
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            ToolResult::error(
                call.id,
                ToolFailure::plain("web_fetch is not wired up yet; answer without it"),
            )
        })
    }
}
