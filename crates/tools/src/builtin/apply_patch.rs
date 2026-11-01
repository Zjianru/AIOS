//! 内置 apply_patch 工具：Codex 风格多文件补丁。
//!
//! 格式与输出照 DSH 的 packages/fs/tool-fs/src/apply-patch*.ts、packages/fs/fs-patch。
//! 整包先解析和预检（不落盘），全部通过才按顺序提交；提交中首个失败即停、不回滚。

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::fs::patch::{self, PatchOp};
use crate::fs::{FileInfo, Target};
use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 内置 apply_patch 工具。
pub struct ApplyPatchTool {
    parameters: Value,
}

impl ApplyPatchTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "input": {"type": "string", "description": "Codex patch text: start with *** Begin Patch and end with *** End Patch. Use *** Add File: path with + lines; *** Update File: path, optionally followed by *** Move to: path, with @@ or @@ context and space-prefixed context, + added, or - removed lines; *** End of File anchors the last hunk to the file end. Use *** Delete File: path to delete. Paths are relative to the Session working directory. Read existing files before updating, deleting, or moving them."}
                },
                "required": ["input"]
            }),
        }
    }
}

impl Default for ApplyPatchTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Create, update, delete, or move UTF-8 text files with one Codex-style patch; prefer it over multiple edit calls when a change spans several files or separated regions."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            match apply_patch(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn apply_patch(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let input = object
        .get("input")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("input must be a string"))?;
    let operations =
        patch::parse(input).map_err(|error| ToolFailure::coded("PATCH_SYNTAX", error.message()))?;
    let planned = plan(operations, context)?;
    let refusals = preflight(context, &planned);
    if !refusals.is_empty() {
        return Err(refusal_error(&refusals));
    }
    commit(context, &planned)
}

/// 一个操作的模型可见记录（index 从 0 开始；渲染时 +1）。
#[derive(Debug, Clone)]
struct OperationRecord {
    index: usize,
    kind: &'static str,
    path: String,
    destination: Option<String>,
    patch_line: usize,
}

struct Planned {
    record: OperationRecord,
    operation: PatchOp,
    target: Target,
    move_target: Option<Target>,
    raw: PathBuf,
}

struct Refusal {
    record: OperationRecord,
    failure: ToolFailure,
}

fn plan(operations: Vec<PatchOp>, context: &ToolContext) -> Result<Vec<Planned>, ToolFailure> {
    let mut planned = Vec::new();
    for (index, operation) in operations.into_iter().enumerate() {
        let (kind, path, patch_line, move_to) = match &operation {
            PatchOp::Add { path, line, .. } => ("add", path.clone(), *line, None),
            PatchOp::Delete { path, line } => ("delete", path.clone(), *line, None),
            PatchOp::Update {
                path,
                line,
                move_to,
                ..
            } => ("update", path.clone(), *line, move_to.clone()),
        };
        let target = crate::fs::resolve(context, &path)?;
        let move_target = match &move_to {
            Some(destination) => Some(crate::fs::resolve(context, destination)?),
            None => None,
        };
        let (record_kind, destination) = match &move_target {
            Some(destination) => ("move", Some(destination.display())),
            None => (kind, None),
        };
        let raw = if std::path::Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            context.cwd.join(&path)
        };
        planned.push(Planned {
            record: OperationRecord {
                index,
                kind: record_kind,
                path: target.display(),
                destination,
                patch_line,
            },
            operation,
            target,
            move_target,
            raw,
        });
    }
    Ok(planned)
}

struct SimulatedFile {
    exists: bool,
    info: Option<FileInfo>,
    content: Option<String>,
    touched: bool,
}

struct Preflight<'a> {
    context: &'a ToolContext,
    states: HashMap<PathBuf, SimulatedFile>,
}

impl<'a> Preflight<'a> {
    fn new(context: &'a ToolContext) -> Self {
        Self {
            context,
            states: HashMap::new(),
        }
    }

    fn state(&mut self, target: &Target) -> Result<&mut SimulatedFile, ToolFailure> {
        if !self.states.contains_key(&target.key) {
            let info = crate::fs::probe(target)?;
            let exists = info.is_some();
            self.states.insert(
                target.key.clone(),
                SimulatedFile {
                    exists,
                    info,
                    content: None,
                    touched: false,
                },
            );
        }
        Ok(self.states.get_mut(&target.key).expect("状态已插入"))
    }

    fn read_content(&mut self, target: &Target) -> Result<String, ToolFailure> {
        if let Some(content) = self.state(target)?.content.clone() {
            return Ok(content);
        }
        let text = crate::fs::read_text_for_verb(target, "patch")?;
        self.state(target)?.content = Some(text.content.clone());
        Ok(text.content)
    }

    fn publish(&mut self, target: &Target, content: Option<String>) {
        self.states.insert(
            target.key.clone(),
            SimulatedFile {
                exists: content.is_some(),
                info: None,
                content,
                touched: true,
            },
        );
    }

    fn check_regular(&mut self, target: &Target) -> Result<(), ToolFailure> {
        if let Some(info) = &self.state(target)?.info
            && !info.is_file
        {
            return Err(ToolFailure::coded(
                "FS_NOT_REGULAR_FILE",
                format!("cannot modify \"{}\": not a regular file", target.display()),
            ));
        }
        Ok(())
    }

    fn check_source(&mut self, planned: &Planned, verb: &str) -> Result<(), ToolFailure> {
        let target = &planned.target;
        if planned.operation_is_move() && crate::fs::is_symlink(&planned.raw) {
            return Err(ToolFailure::coded(
                "PATCH_SYMLINK_SOURCE",
                format!(
                    "cannot {verb} \"{}\": path is a symbolic link — operate on its target path instead",
                    target.display()
                ),
            ));
        }
        let context = self.context;
        let state = self.state(target)?;
        if state.touched {
            if let Some(info) = &state.info
                && !info.is_file
            {
                return Err(ToolFailure::coded(
                    "FS_NOT_REGULAR_FILE",
                    format!("cannot {verb} \"{}\": not a regular file", target.display()),
                ));
            }
            return Ok(());
        }
        let info = state.info.clone();
        crate::fs::check_source(info.as_ref(), false, context, target, verb)
    }

    fn check_add(&mut self, planned: &Planned, content: &str) -> Result<(), ToolFailure> {
        let target = &planned.target;
        self.check_regular(target)?;
        let context = self.context;
        let state = self.state(target)?;
        let touched = state.touched;
        let exists = state.exists;
        let info = state.info.clone();
        crate::fs::check_destination(exists, info.as_ref(), touched, context, target)?;
        self.publish(target, Some(content.to_owned()));
        Ok(())
    }

    fn check_destination_for(
        &mut self,
        destination: &Target,
        content: &str,
    ) -> Result<(), ToolFailure> {
        self.check_regular(destination)?;
        let context = self.context;
        let state = self.state(destination)?;
        let touched = state.touched;
        let exists = state.exists;
        let info = state.info.clone();
        crate::fs::check_destination(exists, info.as_ref(), touched, context, destination)?;
        self.publish(destination, Some(content.to_owned()));
        Ok(())
    }

    fn check(&mut self, planned: &Planned) -> Result<(), ToolFailure> {
        let target = &planned.target;
        match &planned.operation {
            PatchOp::Add { content, .. } => self.check_add(planned, content),
            PatchOp::Delete { .. } => {
                self.check_source(planned, "delete")?;
                self.publish(target, None);
                Ok(())
            }
            PatchOp::Update { hunks, .. } => {
                let verb = if planned.operation_is_move() {
                    "move"
                } else {
                    "edit"
                };
                self.check_source(planned, verb)?;
                let content = self.read_content(target)?;
                let after = patch::apply_hunks(&content, hunks).map_err(|failure| {
                    ToolFailure::coded(
                        "FS_PATCH_MISMATCH",
                        crate::fs::patch_mismatch_message(target, &failure),
                    )
                })?;
                match &planned.move_target {
                    Some(destination) => {
                        self.check_destination_for(destination, &after)?;
                        self.publish(target, None);
                        Ok(())
                    }
                    None => {
                        self.publish(target, Some(after));
                        Ok(())
                    }
                }
            }
        }
    }
}

impl Planned {
    fn operation_is_move(&self) -> bool {
        matches!(
            self.operation,
            PatchOp::Update {
                move_to: Some(_),
                ..
            }
        )
    }
}

fn preflight(context: &ToolContext, operations: &[Planned]) -> Vec<Refusal> {
    let mut preflight = Preflight::new(context);
    let mut refusals = Vec::new();
    for planned in operations {
        if let Err(failure) = preflight.check(planned) {
            refusals.push(Refusal {
                record: planned.record.clone(),
                failure,
            });
        }
    }
    refusals
}

fn refusal_error(refusals: &[Refusal]) -> ToolFailure {
    let first = &refusals[0];
    let lines: Vec<String> = refusals
        .iter()
        .map(|refusal| {
            format!(
                "- operation {} ({} {}, patch line {}): {}",
                refusal.record.index + 1,
                refusal.record.kind,
                refusal.record.path,
                refusal.record.patch_line,
                refusal.failure.message
            )
        })
        .collect();
    ToolFailure {
        code: first.failure.code.clone(),
        message: format!(
            "Patch rejected before any change ({} problems):\n{}",
            refusals.len(),
            lines.join("\n")
        ),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Status {
    Completed,
    Failed,
    Skipped,
}

struct Outcome {
    record: OperationRecord,
    status: Status,
    failure: Option<ToolFailure>,
}

fn commit(context: &ToolContext, planned: &[Planned]) -> Result<String, ToolFailure> {
    let mut outcomes: Vec<Outcome> = planned
        .iter()
        .map(|planned| Outcome {
            record: planned.record.clone(),
            status: Status::Skipped,
            failure: None,
        })
        .collect();
    for (index, planned) in planned.iter().enumerate() {
        match execute_operation(context, planned) {
            Ok(()) => outcomes[index].status = Status::Completed,
            Err(failure) => {
                outcomes[index].status = Status::Failed;
                outcomes[index].failure = Some(failure);
                break;
            }
        }
    }
    if let Some(first) = outcomes.first()
        && first.status == Status::Failed
        && outcomes[1..]
            .iter()
            .all(|outcome| outcome.status == Status::Skipped)
    {
        let failure = first.failure.clone().expect("失败的 outcome 带 failure");
        return Err(refusal_error(&[Refusal {
            record: first.record.clone(),
            failure,
        }]));
    }
    Ok(render(&outcomes))
}

fn execute_operation(context: &ToolContext, planned: &Planned) -> Result<(), ToolFailure> {
    match &planned.operation {
        PatchOp::Add { content, .. } => {
            crate::fs::write_file(context, &planned.target, content)?;
            Ok(())
        }
        PatchOp::Delete { .. } => crate::fs::delete_file(context, &planned.target, "delete"),
        PatchOp::Update {
            move_to: Some(_),
            hunks,
            ..
        } => {
            let destination = planned.move_target.as_ref().expect("move 计划有目的地");
            crate::fs::move_patched_file(context, &planned.target, destination, Some(hunks))
        }
        PatchOp::Update { hunks, .. } => crate::fs::patch_file(context, &planned.target, hunks),
    }
}

fn render(outcomes: &[Outcome]) -> String {
    let stopped = outcomes
        .iter()
        .any(|outcome| outcome.status != Status::Completed);
    if !stopped {
        if outcomes.is_empty() {
            return "Success. No operations in patch.".to_owned();
        }
        let mut lines = vec!["Success. Updated the following files:".to_owned()];
        lines.extend(completed_lines(outcomes));
        return lines.join("\n");
    }
    let completed = outcomes
        .iter()
        .filter(|outcome| outcome.status == Status::Completed)
        .count();
    let mut lines = vec![format!(
        "Patch stopped after {completed} of {} operations.",
        outcomes.len()
    )];
    if completed > 0 {
        lines.push("Completed:".to_owned());
        lines.extend(completed_lines(outcomes));
    }
    for (status, title) in [
        (Status::Failed, "Failed:"),
        (Status::Skipped, "Not executed:"),
    ] {
        let selected: Vec<&Outcome> = outcomes
            .iter()
            .filter(|outcome| outcome.status == status)
            .collect();
        if selected.is_empty() {
            continue;
        }
        lines.push(title.to_owned());
        for outcome in selected {
            let label = format!(
                "- operation {} ({} {}",
                outcome.record.index + 1,
                outcome.record.kind,
                outcome.record.path
            );
            if status == Status::Skipped {
                lines.push(format!("{label})"));
                continue;
            }
            let destination = outcome
                .record
                .destination
                .as_deref()
                .map(|destination| format!(" -> {destination}"))
                .unwrap_or_default();
            let failure = outcome.failure.as_ref().expect("失败的 outcome 带 failure");
            lines.push(format!(
                "{label}{destination}, patch line {}): {}",
                outcome.record.patch_line, failure.message
            ));
        }
    }
    lines.join("\n")
}

fn completed_lines(outcomes: &[Outcome]) -> Vec<String> {
    let mut lines = Vec::new();
    for kind in ["add", "update", "delete"] {
        for outcome in outcomes
            .iter()
            .filter(|outcome| outcome.status == Status::Completed)
        {
            let render_kind = if outcome.record.kind == "move" {
                "update"
            } else {
                outcome.record.kind
            };
            if render_kind != kind {
                continue;
            }
            let path = outcome
                .record
                .destination
                .as_deref()
                .unwrap_or(&outcome.record.path);
            let letter = match kind {
                "add" => "A",
                "delete" => "D",
                _ => "M",
            };
            lines.push(format!("{letter} {path}"));
        }
    }
    lines
}
