//! Kind 的 task_stop：停任务（立即/收尾）。

use super::super::*;
use super::*;

pub struct TaskStopTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskStopTool {
    /// 测试用：直接拿 Kind 的 task_stop 工具入口。
    #[doc(hidden)]
    pub fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "task_id": {"type": "string", "description": "Task id returned by task_accept."},
                    "mode": {
                        "type": "string",
                        "enum": ["now", "after_report"],
                        "description": "now stops the running Attempt immediately. after_report lets it finish the current step, then only task_report is allowed until it reports and ends."
                    }
                },
                "required": ["task_id", "mode"]
            }),
        }
    }
}

impl Tool for TaskStopTool {
    fn name(&self) -> &str {
        "task_stop"
    }

    fn description(&self) -> &str {
        "Stop a running task. mode=now cancels its current Attempt immediately. mode=after_report lets it finish the current step and report once, then it ends; from then on only task_report is allowed. Call it only when the user explicitly asks to stop a task."
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
            let mode = match args.get("mode").and_then(Value::as_str) {
                Some("now") => StopMode::Now,
                Some("after_report") => StopMode::AfterReport,
                _ => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain("mode 必须是 now 或 after_report"),
                    );
                }
            };
            match self.runtime.stop_task(task_id, mode).await {
                Ok(result) => ToolResult::success(call.id, result.message),
                Err(error) => ToolResult::error(call.id, ToolFailure::plain(error.to_string())),
            }
        })
    }
}
