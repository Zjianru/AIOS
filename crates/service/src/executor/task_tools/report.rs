//! ThinAgent 的 task_report：一次报最终状态。

use super::super::*;
use super::*;

/// ThinAgent 的 task_report：一次调用写一条报告并排一条 relay。
pub(crate) struct TaskReportTool {
    runtime: Arc<TaskRuntime>,
    task_id: TaskId,
    attempt_id: AttemptId,
    parameters: Value,
}

impl TaskReportTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>, task_id: TaskId, attempt_id: AttemptId) -> Self {
        Self {
            runtime,
            task_id,
            attempt_id,
            parameters: json!({
                "type": "object",
                "properties": {
                    "state": {"type": "string", "enum": ["ready", "blocked", "failed"], "description": "ready: the DoD is met; blocked: cannot continue without the user or a missing condition; failed: the work cannot produce the result."},
                    "summary": {"type": "string", "description": "The answer or the reason, with the evidence it rests on."},
                    "question": {"type": "string", "description": "One concrete question for the user, only with state=blocked and only when the user must answer it (at most 1000 characters)."}
                },
                "required": ["state", "summary"]
            }),
        }
    }
}

impl Tool for TaskReportTool {
    fn name(&self) -> &str {
        "task_report"
    }

    fn description(&self) -> &str {
        "Report the task's final state once: ready with the result, blocked with what is missing and what the user must do, or failed with what failed. Do not report progress. The user sees it as a card and the Kind conversation receives it as a message."
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
            let state = match args.get("state").and_then(Value::as_str) {
                Some("ready") => ReportState::Ready,
                Some("blocked") => ReportState::Blocked,
                Some("failed") => ReportState::Failed,
                _ => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain("state 必须是 ready、blocked 或 failed"),
                    );
                }
            };
            let Some(summary) = args
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_owned)
            else {
                return ToolResult::error(call.id, ToolFailure::plain("summary 不能为空"));
            };
            let question = args
                .get("question")
                .and_then(Value::as_str)
                .filter(|question| !question.trim().is_empty())
                .map(str::to_owned);
            let conversation = context.session_id().unwrap_or_default().to_owned();
            let report = NewReport {
                state,
                origin: ReportOrigin::Agent,
                summary,
                question,
            };
            match self.runtime.report_from_tool(
                &self.task_id,
                self.attempt_id,
                &conversation,
                report,
            ) {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
