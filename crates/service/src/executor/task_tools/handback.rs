//! ThinAgent 的 task_handback：交回不做了。

use super::super::*;
use super::*;

pub(crate) struct TaskHandbackTool {
    runtime: Arc<TaskRuntime>,
    task_id: TaskId,
    attempt_id: AttemptId,
    parameters: Value,
}

impl TaskHandbackTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>, task_id: TaskId, attempt_id: AttemptId) -> Self {
        Self {
            runtime,
            task_id,
            attempt_id,
            parameters: json!({
                "type": "object",
                "properties": {
                    "reason": {"type": "string", "maxLength": 300, "description": "One sentence saying what the user asked for that is outside this task's scope (at most 300 characters)."}
                },
                "required": ["reason"]
            }),
        }
    }
}

impl Tool for TaskHandbackTool {
    fn name(&self) -> &str {
        "task_handback"
    }

    fn description(&self) -> &str {
        "Hand this task back to Kind when what the user asks is outside the task's scope, belongs to another task, or asks to change the requirement or the DoD. Give one sentence saying what exceeded the task. Do not do that work yourself; the user decides with Kind what happens next."
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
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args = match parse_object(&call.arguments) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            let Some(reason) = args
                .get("reason")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(str::to_owned)
            else {
                return ToolResult::error(call.id, ToolFailure::plain("reason 不能为空"));
            };
            if reason.chars().count() > 300 {
                return ToolResult::error(call.id, ToolFailure::plain("reason 最多 300 字"));
            }
            let conversation = context.session_id().unwrap_or_default().to_owned();
            match self
                .runtime
                .handback(&self.task_id, self.attempt_id, &conversation, reason)
            {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
