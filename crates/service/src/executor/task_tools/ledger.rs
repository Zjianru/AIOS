//! ThinAgent 的账本工具：记一条与读历史。

use super::super::*;
use super::*;

/// ThinAgent 的 task_handback：把超出任务范围、属于别的任务或要改要求/DoD 的话交回 Kind。
///
/// 只 ThinAgent 装；Kind 的工具表里没有它。
/// ：ThinAgent 记账本（否掉的做法 / 下一步）。
pub(crate) struct TaskLedgerNoteTool {
    runtime: Arc<TaskRuntime>,
    task_id: TaskId,
    attempt_id: AttemptId,
    parameters: Value,
}

impl TaskLedgerNoteTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>, task_id: TaskId, attempt_id: AttemptId) -> Self {
        Self {
            runtime,
            task_id,
            attempt_id,
            parameters: json!({
                "type": "object",
                "properties": {
                    "entries": {
                        "type": "array",
                        "minItems": 1,
                        "items": {
                            "type": "object",
                            "properties": {
                                "kind": {"type": "string", "enum": ["rejected", "next"], "description": "rejected: a path you tried and rejected, with the reason; next: what the next attempt should do when you stop or hand back."},
                                "topic": {"type": "string", "description": "A short key for this line; a newer line with the same kind and topic replaces the old one."},
                                "text": {"type": "string", "description": "At most 400 characters; write the fact, not progress."}
                            },
                            "required": ["kind", "topic", "text"]
                        }
                    }
                },
                "required": ["entries"]
            }),
        }
    }
}

impl Tool for TaskLedgerNoteTool {
    fn name(&self) -> &str {
        "task_ledger_note"
    }

    fn description(&self) -> &str {
        "Record durable notes in this task's ledger: a rejected approach with its reason, or what the next attempt should do when you stop or hand back. Use a short topic; a new line with the same kind and topic replaces the old one. Do not record progress here: progress comes from task_report. At most 400 characters per line."
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
            let Some(entries) = args.get("entries").and_then(Value::as_array) else {
                return ToolResult::error(call.id, ToolFailure::plain("entries 必须是数组"));
            };
            if entries.is_empty() {
                return ToolResult::error(call.id, ToolFailure::plain("entries 不能为空"));
            }
            let mut parsed = Vec::with_capacity(entries.len());
            for (index, entry) in entries.iter().enumerate() {
                let Some(object) = entry.as_object() else {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("entries[{index}] 必须是对象")),
                    );
                };
                let kind = match object.get("kind").and_then(Value::as_str) {
                    Some("rejected") => tasks::LedgerNoteKind::Rejected,
                    Some("next") => tasks::LedgerNoteKind::Next,
                    _ => {
                        return ToolResult::error(
                            call.id,
                            ToolFailure::plain(format!(
                                "entries[{index}].kind 必须是 rejected 或 next"
                            )),
                        );
                    }
                };
                let Some(topic) = string_arg(object, "topic") else {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("entries[{index}].topic 不能为空")),
                    );
                };
                let Some(text) = string_arg(object, "text") else {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("entries[{index}].text 不能为空")),
                    );
                };
                parsed.push(tasks::NewLedgerNote { kind, topic, text });
            }
            match self
                .runtime
                .ledger_note_from_tool(&self.task_id, self.attempt_id, &parsed)
            {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// ：ThinAgent 读账本全部历史。
pub(crate) struct TaskLedgerHistoryTool {
    runtime: Arc<TaskRuntime>,
    task_id: TaskId,
    parameters: Value,
}

impl TaskLedgerHistoryTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>, task_id: TaskId) -> Self {
        Self {
            runtime,
            task_id,
            parameters: json!({"type": "object", "properties": {}}),
        }
    }
}

impl Tool for TaskLedgerHistoryTool {
    fn name(&self) -> &str {
        "task_ledger_history"
    }

    fn description(&self) -> &str {
        "Read the task's full ledger history, newest first, including closed lines and why they closed. Use it when you need to remember what was already rejected or what earlier attempts meant to do. It is not injected into your prompt."
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
            match self.runtime.ledger_history_from_tool(&self.task_id) {
                Ok(text) => ToolResult::success(call.id, text),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
