//! 内置 canvas 工具：Kind 在对话里画一块网页（D58–D60；T015.1 只做新建，改同一个画布在 T015.2）。
//!
//! 形状照 Claude Code 的 artifact 工具（`WORK/T015/research/src/claude-code-artifacts.md`）：
//! 工具自己算出这一版的全文，放进 `tool/result.meta.canvas`；模型只看到一句「已显示」。
//! 页面从日志的 meta 取 HTML，重开或刷新都从日志重建。

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolContext, ToolFuture, ToolResult};

/// 画布工具。T015.1 只有新建（不带 id），参数是 title / note / html。
pub struct CanvasTool {
    parameters: Value,
}

impl CanvasTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {
                        "type": "string",
                        "description": "Short title for the canvas header."
                    },
                    "note": {
                        "type": "string",
                        "description": "What this version shows or changes; shown when the version is collapsed."
                    },
                    "html": {
                        "type": "string",
                        "description": "The whole page content (a body fragment); inline its CSS and JavaScript."
                    }
                },
                "required": ["title", "note", "html"],
                "additionalProperties": false
            }),
        }
    }
}

impl Default for CanvasTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for CanvasTool {
    fn name(&self) -> &str {
        "canvas"
    }

    fn description(&self) -> &str {
        describe()
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { create(&call) })
    }
}

fn describe() -> &'static str {
    "Skeleton：工具说明在 T015.1 实现里补齐。"
}

fn create(call: &ToolCall) -> ToolResult {
    let _ = &call.arguments;
    ToolResult::success(call.id.clone(), String::new())
}
