//! Kind 的 task_delivery：读一个任务的交付记录。

use super::super::*;
use super::*;

/// ：Kind 的 task_delivery——读交付记录，只读不出卡。
pub(crate) struct TaskDeliveryTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskDeliveryTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept or a result card."}
                },
                "required": ["task_id"]
            }),
        }
    }
}

impl Tool for TaskDeliveryTool {
    fn name(&self) -> &str {
        "task_delivery"
    }

    fn description(&self) -> &str {
        "Read what a task delivered and why it counts: what was delivered, the evidence, what is still missing, where it went, and whether the user accepted it. The same facts as the task page delivery record. Read-only; report unproven facts as unproven, never as passed."
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
                return ToolResult::error(call.id, ToolFailure::plain("task_id 不能为空"));
            };
            match self.runtime.delivery_text(&task_id).await {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
