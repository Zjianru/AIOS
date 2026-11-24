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
                    "write_scope": {"type": "string", "description": "One path the task may now write: a file, or a directory and everything under it. Write it in full: start with / or ~/; the raw path must appear in the user's message. Relative paths are refused."}
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
        "Change a writing task's write scope after the user agreed in this Kind conversation. The new scope must be written in full (starting with / or ~/) and its raw text must appear in the user's message; otherwise do not call task_update directly: propose_action confirm_write_scope with the scope and task_id, and wait for confirmation. Records a new requirement version and resumes the task in its own conversation with the new scope. Only the write scope can change; anything else needs a new task."
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
            if let Ok(Some(task)) = self
                .runtime
                .tasks_store()
                .task(&tasks::TaskId::from(task_id))
            {
                if let Some(reason) = task.cancelled_message() {
                    return ToolResult::error(call.id, ToolFailure::plain(reason));
                }
                if let Some(reason) = TaskRuntime::trigger_task_rejection(&task) {
                    return ToolResult::error(call.id, ToolFailure::plain(reason));
                }
            }
            let Some(conversation) = context.session_id() else {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain("没有会话 id，不能改写入范围"),
                );
            };
            // 旧 e2e 回放兼容：相对范围按任务 cwd 展开，只在显式测试环境变量下。
            if crate::e2e_legacy_write_scope()
                && !write_scope.starts_with('/')
                && !write_scope.starts_with("~/")
            {
                let base = self
                    .runtime
                    .tasks_store()
                    .task(&tasks::TaskId::from(task_id))
                    .ok()
                    .flatten()
                    .map(|task| task.effective_cwd().to_path_buf())
                    .unwrap_or_else(|| self.runtime.data_dir().to_path_buf());
                let scope = base.join(&write_scope).to_string_lossy().into_owned();
                return match self.runtime.update_scope(task_id, scope).await {
                    Ok(text) => ToolResult::success(call.id, text),
                    Err(failure) => ToolResult::error(call.id, failure),
                };
            }
            if let Err(problem) = crate::write_scope::validate(&write_scope) {
                return ToolResult::error(call.id, ToolFailure::plain(problem));
            }
            let Ok(Some(_)) = crate::ledger::find_user_words(
                &self.runtime.data_dir(),
                conversation,
                write_scope.trim(),
            ) else {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain(format!(
                        "write_scope 不在用户原话里：{write_scope}。不要直接改；调用 propose_action，action=confirm_write_scope，details={{\"write_scope\":\"{write_scope}\",\"task_id\":\"{task_id}\"}}，让用户确认新的写入范围。"
                    )),
                );
            };
            let scope = match crate::write_scope::expand(&write_scope) {
                Ok(scope) => scope,
                Err(problem) => return ToolResult::error(call.id, ToolFailure::plain(problem)),
            };
            match self
                .runtime
                .update_scope(task_id, scope.to_string_lossy().into_owned())
                .await
            {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
