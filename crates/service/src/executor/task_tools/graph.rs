//! Kind 的 task_graph_propose：提议任务图与解析图参数。

use super::super::*;
use super::*;

/// Kind 的 task_graph_propose：有先后的几件事建一张任务图。
///
/// 校验（至少两项、after 只指本提议、无环、没有先后的写入范围不重叠）在 tasks 里；
/// 校验过了建图并回图 id，Kind 再调 propose_action（action=start_task_graph）出确认卡。
pub(crate) struct TaskGraphProposeTool {
    runtime: Arc<TaskRuntime>,
    parameters: Value,
}

impl TaskGraphProposeTool {
    pub(crate) fn new(runtime: Arc<TaskRuntime>) -> Self {
        Self {
            runtime,
            parameters: json!({
                "type": "object",
                "properties": {
                    "title": {"type": "string", "description": TASK_ACCEPT_TITLE_DESCRIPTION},
                    "quote": {"type": "string", "description": "The user's request, copied verbatim from a user message. Required. The whole call is rejected if this is not a contiguous substring of a user message; on rejection, copy the exact words again or ask the user, never invent or paraphrase."},
                    "kind_note": {"type": "string", "description": "Optional note from Kind: assumptions or additions that are not the user's own words. Keep it out of quote and requirement."},
                    "requirement": {"type": "string", "description": "Kind's understanding of what the whole graph must achieve, not a copy of the user's words."},
                    "dod": {"type": "string", "description": "How the whole graph will be judged complete."},
                    "cwd": {"type": "string", "description": "The starting path the user pointed at (a file or directory). Omit for a read-only task to use the project's default directory, then $HOME; write-task placement is settled by its write scope. A path you pass must already exist; only pass a path the user actually pointed at, never infer one."},
                    "project": {"type": "string", "description": "Project id from the fixed block's project list for the whole graph. Omit for a personal graph or when the user did not name a project, even when the block lists exactly one project; never auto-assign the only project. Ask the user if unsure. Never infer it from cwd. If the project's default directory does not exist, tell the user and ask what to do. Any directory you pass must be one the user pointed at."},
                    "constraints": {"type": "array", "items": {"type": "string"}, "description": "The user's hard requirements for the whole graph, each copied verbatim from a user message. Omit when none. Checked against the originating conversation; one non-verbatim string rejects the whole call."},
                    "subtasks": {
                        "type": "array",
                        "minItems": 2,
                        "description": "The tasks in order. Put the step that must happen first earlier or link it with after.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "title": {"type": "string", "description": TASK_ACCEPT_TITLE_DESCRIPTION},
                                "requirement": {"type": "string", "description": "Kind's understanding of what this step must do; not a copy of the user's words."},
                                "dod": {"type": "string", "description": "How this step will be judged complete."},
                                "write_scope": {"type": "string", "description": "One path this step may write, written in full starting with / or ~/; the raw path must appear in the user message. Omit for a read-only step."},
                                "worktree": {"type": "boolean", "description": "Only when the user explicitly asks for a worktree. Defaults to false."},
                                "after": {"type": "array", "items": {"type": "integer"}, "description": "Indexes (0-based) of subtasks in this proposal that must have a result before this step starts."},
                                "milestone": {"type": "boolean", "description": "True for a read-only check step the user judges: its after has exactly one index (the step it checks), it cannot write, and the user passes it or sends it back for fixes on its result card. Use it when the user says they want to look before continuing. Do not combine with program_gate."},
                                "program_gate": {"type": "object", "description": "A read-only program gate that runs these commands in order after the checked step reports. Use only when the user names commands that decide pass/fail (all exit codes 0 = pass). Mutually exclusive with milestone.", "properties": {"commands": {"type": "array", "minItems": 1, "items": {"type": "object", "properties": {"command": {"type": "string", "description": "The command exactly as the user said it, in bash -c syntax; do not rewrite or add arguments."}, "timeout_ms": {"type": "integer", "description": "Only set when the user named a timeout; otherwise omit (default 120 seconds, max 600000)."}}, "required": ["command"]}}}, "required": ["commands"]}
                            },
                            "required": ["title", "requirement", "dod"]
                        }
                    }
                },
                "required": ["title", "quote", "requirement", "dod", "subtasks"]
            }),
        }
    }
}

impl Tool for TaskGraphProposeTool {
    fn name(&self) -> &str {
        "task_graph_propose"
    }

    fn description(&self) -> &str {
        "Propose one ordered task graph when the user's request has steps that must run in order. Always pass quote on the root: the user's request copied verbatim from a user message, never paraphrased; the call is rejected if it is not a contiguous substring of a user message. Child tasks inherit the root quote. Put Kind's understanding in requirement and any Kind-only addition in kind_note. Creates the graph and returns its id; then call propose_action once with action start_task_graph and that id so the user confirms on the card. Pass project only when the whole graph clearly belongs to one project; ask the user if unsure and never infer it from cwd. Independent work belongs in task_accept, not here."
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
            let parsed = match parse_graph_args(&args) {
                Ok(parsed) => parsed,
                Err(failure) => return ToolResult::error(call.id, failure),
            };
            match self.runtime.propose_graph(context, &call.id, parsed) {
                Ok((root, count)) => ToolResult::success(
                    call.id,
                    format!(
                        "已建任务图 {}（{} 项）。现在调用 propose_action 一次：options 里一项 action=start_task_graph，details.task_id 填 {}；确认和暂自由用户在卡片上点。确认后没有先后的子任务先跑，等上游的子任务会自动开跑，不要再单独派它们。",
                        root.id, count, root.id
                    ),
                ),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

/// ：解析 task_graph_propose 的参数（字段检查照 task_accept；图语义由 tasks 校验）。
fn parse_program_gate(
    value: Option<&Value>,
    index: usize,
) -> Result<Vec<GateCommand>, ToolFailure> {
    let Some(value) = value.filter(|value| !value.is_null()) else {
        return Ok(Vec::new());
    };
    let object = value
        .as_object()
        .ok_or_else(|| ToolFailure::plain(format!("subtasks[{index}].program_gate 必须是对象")))?;
    let Some(commands) = object.get("commands").and_then(Value::as_array) else {
        return Err(ToolFailure::plain(format!(
            "subtasks[{index}].program_gate.commands 必须是数组"
        )));
    };
    let mut parsed = Vec::with_capacity(commands.len());
    for (item_index, item) in commands.iter().enumerate() {
        let Some(object) = item.as_object() else {
            return Err(ToolFailure::plain(format!(
                "subtasks[{index}].program_gate.commands[{item_index}] 必须是对象"
            )));
        };
        let Some(command) = object.get("command").and_then(Value::as_str) else {
            return Err(ToolFailure::plain(format!(
                "subtasks[{index}].program_gate.commands[{item_index}].command 不能为空"
            )));
        };
        let timeout_ms = match object.get("timeout_ms") {
            None | Some(Value::Null) => None,
            Some(Value::Number(number)) => Some(
                number
                    .as_u64()
                    .ok_or_else(|| ToolFailure::plain("timeout_ms 必须是非负整数"))?,
            ),
            Some(_) => return Err(ToolFailure::plain("timeout_ms 必须是整数")),
        };
        parsed.push(GateCommand {
            command: command.trim().to_owned(),
            timeout_ms,
        });
    }
    tasks::validate_gate_commands(&parsed)
        .map_err(|error| ToolFailure::plain(error.to_string()))?;
    Ok(parsed)
}

fn parse_graph_args(args: &serde_json::Map<String, Value>) -> Result<TaskGraphArgs, ToolFailure> {
    let title = string_arg(args, "title").ok_or_else(|| ToolFailure::plain("title 不能为空"))?;
    let quote = string_arg(args, "quote").ok_or_else(|| ToolFailure::plain("quote 不能为空"))?;
    if quote.trim().is_empty() {
        return Err(ToolFailure::plain("quote 不能为空"));
    }
    let kind_note = match args.get("kind_note") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()).filter(|value| !value.trim().is_empty()),
        Some(_) => return Err(ToolFailure::plain("kind_note 必须是字符串")),
    };
    let requirement = string_arg(args, "requirement")
        .ok_or_else(|| ToolFailure::plain("requirement 不能为空"))?;
    let dod = string_arg(args, "dod").ok_or_else(|| ToolFailure::plain("dod 不能为空"))?;
    let cwd = args
        .get("cwd")
        .and_then(Value::as_str)
        .filter(|cwd| !cwd.trim().is_empty())
        .map(str::to_owned);
    let project = match args.get("project") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()).filter(|value| !value.trim().is_empty()),
        Some(_) => return Err(ToolFailure::plain("project 必须是字符串")),
    };
    let raw_subtasks = args
        .get("subtasks")
        .and_then(Value::as_array)
        .ok_or_else(|| ToolFailure::plain("subtasks 必须是数组"))?;
    if raw_subtasks.len() < 2 {
        return Err(ToolFailure::plain("任务图至少要两个子任务"));
    }
    let mut subtasks = Vec::with_capacity(raw_subtasks.len());
    for (index, value) in raw_subtasks.iter().enumerate() {
        let object = value
            .as_object()
            .ok_or_else(|| ToolFailure::plain(format!("subtasks[{index}] 必须是对象")))?;
        let subtask_title = string_arg(object, "title")
            .ok_or_else(|| ToolFailure::plain(format!("subtasks[{index}].title 不能为空")))?;
        let subtask_requirement = string_arg(object, "requirement")
            .ok_or_else(|| ToolFailure::plain(format!("subtasks[{index}].requirement 不能为空")))?;
        let subtask_dod = string_arg(object, "dod")
            .ok_or_else(|| ToolFailure::plain(format!("subtasks[{index}].dod 不能为空")))?;
        let write_scope = match object.get("write_scope") {
            None | Some(Value::Null) => None,
            Some(Value::String(value)) => {
                Some(value.clone()).filter(|value| !value.trim().is_empty())
            }
            Some(_) => {
                return Err(ToolFailure::plain(format!(
                    "subtasks[{index}].write_scope 必须是字符串"
                )));
            }
        };
        let worktree = match object.get("worktree") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(ToolFailure::plain(format!(
                    "subtasks[{index}].worktree 必须是布尔值"
                )));
            }
        };
        let after = match object.get("after") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => {
                let mut after = Vec::with_capacity(items.len());
                for item in items {
                    let Some(value) = item.as_u64() else {
                        return Err(ToolFailure::plain(format!(
                            "subtasks[{index}].after 里的项必须是整数"
                        )));
                    };
                    after.push(usize::try_from(value).map_err(|_| {
                        ToolFailure::plain(format!("subtasks[{index}].after 里的下标太大：{value}"))
                    })?);
                }
                after
            }
            Some(_) => {
                return Err(ToolFailure::plain(format!(
                    "subtasks[{index}].after 必须是数组"
                )));
            }
        };
        let milestone = match object.get("milestone") {
            None => false,
            Some(Value::Bool(value)) => *value,
            Some(_) => {
                return Err(ToolFailure::plain(format!(
                    "subtasks[{index}].milestone 必须是布尔值"
                )));
            }
        };
        let program_gate = parse_program_gate(object.get("program_gate"), index)?;
        if milestone && !program_gate.is_empty() {
            return Err(ToolFailure::plain(format!(
                "subtasks[{index}] 不能同时是用户判里程碑和程序门"
            )));
        }
        let milestone = milestone || !program_gate.is_empty();
        subtasks.push(GraphSubtaskArgs {
            title: subtask_title,
            requirement: subtask_requirement,
            dod: subtask_dod,
            write_scope,
            worktree,
            after,
            milestone,
            program_gate,
        });
    }
    Ok(TaskGraphArgs {
        title,
        quote,
        requirement,
        dod,
        kind_note,
        cwd,
        project,
        subtasks,
        constraints: parse_constraints(args)?,
    })
}
