//! 内置 edit 工具：字面替换（默认唯一匹配）。
//!
//! 参数、输出、错误文字照 DSH 的 packages/fs/tool-fs/src/edit.ts。

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 内置 edit 工具。
pub struct EditTool {
    parameters: Value,
}

impl EditTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Path to edit, resolved by the filesystem backend. Provide `file_path` before `old_string` and `new_string` in the arguments."},
                    "old_string": {"type": "string", "description": "Literal text to replace."},
                    "new_string": {"type": "string", "description": "Literal replacement text. Use an empty string to delete the match."},
                    "replace_all": {"type": "boolean", "description": "Replace all matches. Defaults to false; when false, old_string must appear exactly once."}
                },
                "required": ["file_path", "old_string", "new_string"]
            }),
        }
    }
}

impl Default for EditTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for EditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        "Edit an existing UTF-8 text file by replacing literal text."
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
            match edit(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn edit(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
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
    let old_string = object
        .get("old_string")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("old_string must be a non-empty string"))?;
    if old_string.is_empty() {
        return Err(ToolFailure::plain("old_string must be a non-empty string"));
    }
    let new_string = object
        .get("new_string")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("new_string must be a string"))?;
    if old_string == new_string {
        return Err(ToolFailure::plain("old_string and new_string must differ"));
    }
    let replace_all = match object.get("replace_all") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| ToolFailure::plain("replace_all must be a boolean"))?,
    };
    let target = crate::fs::resolve(context, file_path)?;
    crate::fs::edit_file(context, &target, old_string, new_string, replace_all)?;
    let display = target.display();
    Ok(if replace_all {
        format!("The file {display} has been updated. All occurrences were successfully replaced.")
    } else {
        format!("The file {display} has been updated successfully.")
    })
}
