//! Kind 的 gate_check_again：重跑程序门命令。

use super::super::*;
use super::*;

/// Kind 的 task_stop：页面 TaskStop 调的是同一个 stop_task。
/// ：Kind 的 gate_check_again——程序门未确定/被打断时直接重跑，不再出卡。
pub(crate) struct GateCheckAgainTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl GateCheckAgainTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "gate_task_id": {"type": "string", "description": "The program gate task id whose latest check was undetermined or interrupted."}
                },
                "required": ["gate_task_id"]
            }),
        }
    }
}

impl Tool for GateCheckAgainTool {
    fn name(&self) -> &str {
        "gate_check_again"
    }

    fn description(&self) -> &str {
        "Re-run a program gate's commands for the current candidate when the last check was undetermined or interrupted and the commands have not changed. This is already authorized by the user's earlier confirmation; it does not show a card. Refuse and explain when the gate is a user-judged milestone, the last conclusion was pass/needs_fix, the candidate changed, or the command version changed."
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
            let Some(gate_task_id) = string_arg(&args, "gate_task_id") else {
                return ToolResult::error(call.id, ToolFailure::plain("gate_task_id 不能为空"));
            };
            match self
                .runtime
                .gate_check_again(&TaskId::from(gate_task_id.as_str()))
            {
                Ok(()) => ToolResult::success(
                    call.id,
                    format!("已重排程序门检查（门 {gate_task_id}）；结果会作为结果卡送到这里。"),
                ),
                Err(error) => ToolResult::error(call.id, ToolFailure::plain(error)),
            }
        })
    }
}
