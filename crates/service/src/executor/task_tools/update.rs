//! Kind 的 task_update：改任务状态或要求。

use super::super::*;
use super::*;

/// Kind 的 task_update：用户同意后换写入范围，出新要求版本并接回原任务会话。
pub(crate) struct TaskUpdateTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskUpdateTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept."},
                    "write_scope": {"type": "string", "description": "One path the task may now write: a file, or a directory and everything under it. Relative paths resolve against the task's cwd."}
                },
                "required": ["task_id", "write_scope"]
            }),
        }
    }
}

impl Tool for TaskUpdateTool {
    fn name(&self) -> &str {
        "task_update"
    }

    fn description(&self) -> &str {
        "Change a writing task's write scope after the user agreed in this Kind conversation. Records a new requirement version and resumes the task in its own conversation with the new scope. Only the write scope can change; anything else needs a new task."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn condition(&self) -> ToolCondition {
        ToolCondition::Always
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(task_id) = args.get("task_id").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("task_id 不能为空"));
            };
            let Some(write_scope) = args
                .get("write_scope")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|scope| !scope.is_empty())
                .map(str::to_owned)
            else {
                return ToolResult::error(call.id, ToolFailure::plain("write_scope 不能为空"));
            };
            match self.runtime.update_scope(task_id, write_scope).await {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
