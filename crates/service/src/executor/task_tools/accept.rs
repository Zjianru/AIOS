//! Kind 的 task_accept：把只读调查派成任务。

use super::super::*;
use super::*;

/// task_accept 的 title 参数说明：卡片标题的硬上限写进 schema，模型生成时就被约束。
pub(crate) const TASK_ACCEPT_TITLE_DESCRIPTION: &str = "Short label for the card title: at most 20 characters (Chinese characters count as one); this is the card title; longer titles are truncated.";

/// Kind 的 task_accept。
pub(crate) struct TaskAcceptTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

/// 解析 task_accept 参数（工具和确认卡共用同一份字段规则）。
pub(crate) fn parse_accept_args(args: &Value) -> Result<TaskAcceptArgs, ToolFailure> {
    let args = args
        .as_object()
        .ok_or_else(|| ToolFailure::plain("arguments must be a JSON object"))?;
    let Some(title) = string_arg(args, "title") else {
        return Err(ToolFailure::plain("title 不能为空"));
    };
    let Some(quote) = string_arg(args, "quote") else {
        return Err(ToolFailure::plain("quote 不能为空"));
    };
    if quote.trim().is_empty() {
        return Err(ToolFailure::plain("quote 不能为空"));
    }
    let Some(requirement) = string_arg(args, "requirement") else {
        return Err(ToolFailure::plain("requirement 不能为空"));
    };
    let Some(dod) = string_arg(args, "dod") else {
        return Err(ToolFailure::plain("dod 不能为空"));
    };
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty())
        .map(str::to_owned);
    let write_scope = match args.get("write_scope") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()).filter(|value| !value.trim().is_empty()),
        Some(_) => {
            return Err(ToolFailure::plain("write_scope 必须是字符串"));
        }
    };
    let project = match args.get("project") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()).filter(|value| !value.trim().is_empty()),
        Some(_) => {
            return Err(ToolFailure::plain("project 必须是字符串"));
        }
    };
    let worktree = match args.get("worktree") {
        None => false,
        Some(Value::Bool(value)) => *value,
        Some(_) => {
            return Err(ToolFailure::plain("worktree 必须是布尔值"));
        }
    };
    let kind_note = match args.get("kind_note") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()).filter(|value| !value.trim().is_empty()),
        Some(_) => {
            return Err(ToolFailure::plain("kind_note 必须是字符串"));
        }
    };
    let constraints = parse_constraints(args)?;
    Ok(TaskAcceptArgs {
        title,
        quote,
        requirement,
        kind_note,
        dod,
        cwd,
        project,
        write_scope,
        worktree,
        constraints,
    })
}

impl TaskAcceptTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": TASK_ACCEPT_TITLE_DESCRIPTION},
                    "quote": {"type": "string", "description": "The user's request, copied verbatim from a user message. Required. The whole call is rejected if this is not a contiguous substring of a user message; on rejection, copy the exact words again or ask the user, never invent or paraphrase."},
                    "kind_note": {"type": "string", "description": "Optional note from Kind: assumptions or additions that are not the user's own words. Keep it out of quote and requirement."},
                    "requirement": {"type": "string", "description": "Kind's understanding of what the task must do, not a copy of the user's words."},
                    "dod": {"type": "string", "description": "How the result will be judged complete."},
                    "cwd": {"type": "string", "description": "The starting path the user pointed at (a file or directory). Omit for a read-only task to use the project's default directory, then $HOME; write-task placement is settled by its write scope. A path you pass must already exist; only pass a path the user actually pointed at, never infer one."},
                    "project": {"type": "string", "description": "Project id from the fixed block's project list that this task belongs to. Omit for personal tasks or when you are unsure; if unsure, ask the user (include the personal choice) before dispatching. If the user did not name a project, omit this field even when the fixed block lists exactly one project; never auto-assign a task to the only project. Never infer a project from cwd. If the project's default directory does not exist, tell the user and ask what to do. Any directory you pass must be one the user pointed at."},
                    "write_scope": {"type": "string", "description": "One path this task may write: a file, or a directory and everything under it. Write it in full, starting with / or ~/; copy it exactly from the user's message, including a leading ~/. If the user named a path, pass that exact raw path. If the user wants a file written but did not name a path, do not dispatch and do not call ask_user_question: first load skill aios-tasks, then propose_action once with action confirm_write_scope, write_scope set to one concrete / or ~/ path you propose, and task set to the full task_accept arguments; let the user confirm where to write. If the user named a path but did not say what to write, ask what to write instead of dispatching. Omit for a read-only investigation."},
                    "worktree": {"type": "boolean", "description": "Run the task in a new git worktree under the data directory. Only when the user explicitly asks for a worktree. Defaults to false."},
                    "constraints": {"type": "array", "items": {"type": "string"}, "description": "The user's hard requirements (must/never rules), each copied verbatim from a user message. Omit when the user stated none. Every string is checked against the originating conversation and the whole call is rejected if one is not verbatim user text."}
                },
                "required": ["title", "quote", "requirement", "dod"]
            }),
        }
    }
}

impl Tool for TaskAcceptTool {
    fn name(&self) -> &str {
        "task_accept"
    }

    fn description(&self) -> &str {
        "When the user wants a file changed (even a single typo), a command run, or an investigation that reads more than one or two files, call task_accept to dispatch a task; then reply in the user's language with what you dispatched and the result you expect. Always pass quote: the user's request copied verbatim from a user message, never paraphrased; the call is rejected if it is not a contiguous substring of a user message. Put Kind's understanding in requirement and any Kind-only addition in kind_note, not in quote. Omit write_scope for a read-only investigation; for a writing task follow the write_scope description. If the user wants a file written but gave no path, do not ask for content/audience/format and do not call ask_user_question: load aios-tasks, then propose confirm_write_scope with one concrete path. Use ask_user_question only when the user gave a path but not yet what to write. Pass project only when the task clearly belongs to one of the fixed block's projects; ask the user if unsure and never infer it from cwd. Pass worktree=true only when the user explicitly asked for a git worktree. The result arrives later as a task_report message; the user already sees a card. If the user later asks how a task is going, call task_status once before answering."
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
            let raw: Value = match serde_json::from_str(&call.arguments) {
                Ok(value) => value,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("arguments must be a JSON object: {error}")),
                    );
                }
            };
            let args = match parse_accept_args(&raw) {
                Ok(args) => args,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.runtime.accept(context, &call.id, args, false) {
                Ok((task, true)) => ToolResult::success(
                    call.id,
                    format!(
                        "已派出任务 {}。结果会作为 task_report 消息送来；用户已经看到卡片，只按需回答。",
                        task.id
                    ),
                ),
                Ok((task, false)) => ToolResult::success(
                    call.id,
                    format!(
                        "已受理过（Already accepted as {}）；没有重复派出。结果会作为 task_report 消息送来。",
                        task.id
                    ),
                ),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}
