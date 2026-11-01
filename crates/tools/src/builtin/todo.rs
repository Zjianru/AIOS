//! 内置 todo 工具：整表替换的 todo_write（照 DSH todo/tool-todo）。

use std::collections::BTreeSet;

use serde_json::{Value, json};
use session::{TodoItem, TodoStatus};

use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// todo 列表维护工具。
pub struct TodoWriteTool {
    parameters: Value,
}

impl TodoWriteTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "todos": {
                        "type": "array",
                        "description": "The COMPLETE task list, replacing any previous list.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "content": {"type": "string", "description": "What the task is — a short imperative line."},
                                "status": {"type": "string", "enum": ["pending", "in_progress", "completed"], "description": "pending (not started) | in_progress (now) | completed (done)."}
                            },
                            "required": ["content", "status"],
                            "additionalProperties": false
                        }
                    }
                },
                "required": ["todos"]
            }),
        }
    }
}

impl Default for TodoWriteTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todo_write"
    }

    fn description(&self) -> &str {
        "Record and update a task list to plan multi-step work and show progress; skip it for trivial single-step tasks. Add one todo per concrete step before you start. While work remains, keep the todos being worked on `in_progress`, several only when work runs in parallel. Mark each todo `completed` as soon as it is done."
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
    if context.session().session_id().is_none() {
        return Err(ToolFailure::plain(
            "todo_write requires an owning agent session",
        ));
    }
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let Some(items) = object.get("todos") else {
        return Err(ToolFailure::plain("invalid todos: missing `todos`"));
    };
    let Some(items) = items.as_array() else {
        return Err(ToolFailure::plain("invalid todos: expected an array"));
    };
    let mut todos = Vec::with_capacity(items.len());
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for item in items {
        let Some(item) = item.as_object() else {
            return Err(ToolFailure::plain("invalid todo: expected an object"));
        };
        for key in item.keys() {
            if key != "content" && key != "status" {
                return Err(ToolFailure::plain(format!(
                    "invalid todos: unknown property \"{key}\""
                )));
            }
        }
        let Some(content) = item.get("content").and_then(Value::as_str) else {
            return Err(ToolFailure::plain(
                "invalid todo: `content` must be a non-empty string",
            ));
        };
        let content = content.trim();
        if content.is_empty() {
            return Err(ToolFailure::plain(
                "invalid todo: `content` must be a non-empty string",
            ));
        }
        if !seen.insert(content.to_owned()) {
            return Err(ToolFailure::plain(format!(
                "invalid todos: duplicate content \"{content}\""
            )));
        }
        let Some(status) = item
            .get("status")
            .and_then(Value::as_str)
            .and_then(TodoStatus::parse)
        else {
            return Err(ToolFailure::plain(
                "invalid todos: status must be pending, in_progress or completed",
            ));
        };
        todos.push(TodoItem {
            content: content.to_owned(),
            status,
        });
    }
    let pending = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::Pending)
        .count();
    let in_progress = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::InProgress)
        .count();
    let completed = todos
        .iter()
        .filter(|todo| todo.status == TodoStatus::Completed)
        .count();
    context.session().set_todos(todos);
    Ok(format!(
        "Updated todo list: {pending} pending, {in_progress} in progress, {completed} completed."
    ))
}
