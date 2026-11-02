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
    // T006.6：解析/预检的所有拒绝都汇总进同一条 Patch rejected（DSH patch/index.ts:31-38：
    // plan refusals 与 preflight refusals 合并后按操作序号排序）。
    let (planned, mut refusals) = plan(operations, context);
    refusals.extend(preflight(context, &planned));
    refusals.sort_by_key(|refusal| refusal.record.index);
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

fn plan(operations: Vec<PatchOp>, context: &ToolContext) -> (Vec<Planned>, Vec<Refusal>) {
    let mut planned = Vec::new();
    let mut refusals = Vec::new();
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
        let mut record = OperationRecord {
            index,
            kind,
            path: path.clone(),
            destination: None,
            patch_line,
        };
        let target = match crate::fs::resolve(context, &path) {
            Ok(target) => target,
            Err(failure) => {
                refusals.push(Refusal { record, failure });
                continue;
            }
        };
        record.path = target.display();
        let move_target = match &move_to {
            Some(destination) => match crate::fs::resolve(context, destination) {
                Ok(destination) => {
                    record.kind = "move";
                    record.destination = Some(destination.display());
                    Some(destination)
                }
                Err(failure) => {
                    record.kind = "move";
                    refusals.push(Refusal { record, failure });
                    continue;
                }
            },
            None => None,
        };
        let raw = if std::path::Path::new(&path).is_absolute() {
            PathBuf::from(&path)
        } else {
            context.cwd.join(&path)
        };
        planned.push(Planned {
            record,
            operation,
            target,
            move_target,
            raw,
        });
    }
    (planned, refusals)
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
        // DSH preflight.ts:93-97：delete 和 move 都拒绝符号链接源（edit 不移动/删除路径本身）。
        if verb != "edit" && crate::fs::is_symlink(&planned.raw) {
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
                // DSH preflight.ts:90-92：move 的源和目的地是同一个 targetKey 时直接拒。
                if let Some(destination) = &planned.move_target
                    && destination.key == planned.target.key
                {
                    return Err(ToolFailure::coded(
                        "FS_SAME_TARGET",
                        format!("cannot move \"{}\" to itself", target.display()),
                    ));
                }
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
    /// 失败前是否已经产生可观察的部分效果（目前只有 move 的「目的地已写入」）。
    partial: bool,
}

/// 一次操作失败：partial 决定能不能说「什么都没改」。
struct OperationFailure {
    failure: ToolFailure,
    partial: bool,
}

impl OperationFailure {
    fn clean(failure: ToolFailure) -> Self {
        Self {
            failure,
            partial: false,
        }
    }
}

fn commit(context: &ToolContext, planned: &[Planned]) -> Result<String, ToolFailure> {
    let mut outcomes: Vec<Outcome> = planned
        .iter()
        .map(|planned| Outcome {
            record: planned.record.clone(),
            status: Status::Skipped,
            failure: None,
            partial: false,
        })
        .collect();
    for (index, planned) in planned.iter().enumerate() {
        match execute_operation(context, planned) {
            Ok(()) => outcomes[index].status = Status::Completed,
            Err(failure) => {
                outcomes[index].status = Status::Failed;
                outcomes[index].partial = failure.partial;
                outcomes[index].failure = Some(failure.failure);
                break;
            }
        }
    }
    // 只有「首个操作失败且确实什么都没改」才按整包拒绝报；有部分效果的走 stopped 文本。
    if let Some(first) = outcomes.first()
        && first.status == Status::Failed
        && !first.partial
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

fn execute_operation(context: &ToolContext, planned: &Planned) -> Result<(), OperationFailure> {
    match &planned.operation {
        PatchOp::Add { content, .. } => crate::fs::write_file(context, &planned.target, content)
            .map(|_| ())
            .map_err(OperationFailure::clean),
        PatchOp::Delete { .. } => crate::fs::delete_file(context, &planned.target, "delete")
            .map_err(OperationFailure::clean),
        PatchOp::Update {
            move_to: Some(_),
            hunks,
            ..
        } => {
            let destination = planned.move_target.as_ref().expect("move 计划有目的地");
            crate::fs::move_patched_file(context, &planned.target, destination, Some(hunks))
                .map_err(|failure| OperationFailure {
                    failure: failure.failure,
                    partial: failure.destination_written,
                })
        }
        PatchOp::Update { hunks, .. } => {
            crate::fs::patch_file(context, &planned.target, hunks).map_err(OperationFailure::clean)
        }
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
            // DSH apply-patch-output.ts:122-123：部分 move 单独一行说明目的地已写、源还在。
            if outcome.partial && outcome.record.destination.is_some() {
                lines.push(format!(
                    "  destination {} was written; source {} remains",
                    outcome.record.destination.as_deref().unwrap_or_default(),
                    outcome.record.path
                ));
            }
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
