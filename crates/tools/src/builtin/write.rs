//! 内置 write 工具：整文件创建或覆盖。
//!
//! 参数、信封、错误文字照 DSH 的 packages/fs/tool-fs/src/write.ts。

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 内置 write 工具。
pub struct WriteTool {
    parameters: Value,
}

impl WriteTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Path to write, resolved by the filesystem backend. Provide `file_path` before `content` in the arguments."},
                    "content": {"type": "string", "description": "Full UTF-8 text content to write."}
                },
                "required": ["file_path", "content"]
            }),
        }
    }
}

impl Default for WriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for WriteTool {
    fn name(&self) -> &str {
        "write"
    }

    fn description(&self) -> &str {
        "Create or fully replace a UTF-8 text file."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            match write(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn write(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let file_path = object
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("file_path must be a non-empty string"))?;
    if file_path.trim().is_empty() {
        return Err(ToolFailure::plain("file_path must be a non-empty string"));
    }
    let content = object
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("content must be a string"))?;
    let target = crate::fs::resolve(context, file_path)?;
    let created = crate::fs::write_file(context, &target, content)?;
    let verb = if created { "Created" } else { "Updated" };
    Ok(format!(
        "<path>{}</path>\n<type>file</type>\n<content>\n{verb} file\n</content>",
        target.display()
    ))
}
