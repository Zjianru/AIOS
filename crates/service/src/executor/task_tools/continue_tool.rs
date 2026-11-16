//! Kind 的 task_continue：让任务在原会话接着做。

use super::super::*;
use super::*;

/// Kind 的 task_continue：让任务在原要求里接着做。
pub(crate) struct TaskContinueTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskContinueTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "The id of the task that must continue in its original requirement (the step the milestone checks, or a task whose last report was blocked/failed)."},
                    "note": {"type": "string", "description": "The user's words to pass on, verbatim. Omit when the user did not write anything specific."}
                },
                "required": ["task_id"]
            }),
        }
    }
}

impl Tool for TaskContinueTool {
    fn name(&self) -> &str {
        "task_continue"
    }

    fn description(&self) -> &str {
        "Let a task continue in its original requirement and write scope as a new Attempt: use it when the user sends a milestone back needing fixes, or when a task's last report was blocked/failed. This does not change the requirement or write scope; to change those, ask the user first. If the user's words go beyond the original requirement, do not call this: ask the user to change the requirement."
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
            let Some(task_id) = string_arg(&args, "task_id") else {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain("task_id 不能为空".to_owned()),
                );
            };
            let note = args
                .get("note")
                .and_then(Value::as_str)
                .filter(|note| !note.trim().is_empty())
                .map(str::to_owned);
            match self
                .runtime
                .continue_task(crate::protocol::TaskContinueRequest { task_id, note })
                .await
            {
                Ok(response) => ToolResult::success(
                    call.id,
                    format!(
                        "已让任务在原要求里接着做（第 {} 次 Attempt）；用户原话和核查发现已放进它的任务包。",
                        response.attempt_id
                    ),
                ),
                Err(reason) => ToolResult::error(call.id, ToolFailure::plain(reason)),
            }
        })
    }
}
