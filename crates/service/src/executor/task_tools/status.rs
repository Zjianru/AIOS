//! Kind 的 task_status：问任务当前进展。

use super::super::*;
use super::*;

/// Kind 的 task_status。
pub(crate) struct TaskStatusTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskStatusTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept."}
                },
                "required": ["task_id"]
            }),
        }
    }
}

impl Tool for TaskStatusTool {
    fn name(&self) -> &str {
        "task_status"
    }

    fn description(&self) -> &str {
        "Read a task's stage, its latest report in full, and its Attempt count. While the task is waiting for a report, this also asks its ThinAgent once for a status report."
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
            match self.runtime.status_text(task_id).await {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
