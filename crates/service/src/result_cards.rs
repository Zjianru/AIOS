//! 写任务的结果卡：改动清单和固定文字。
//!
//! 清单取任务所有 Attempt 里 write、edit、apply_patch 成功结果的 `meta.diffs[].path`，
//! 去重后相对 cwd 显示；只读调查任务没有清单。

use std::collections::HashSet;
use std::path::{Component, Path};

use model::{ContentBlock, MessageSource};
use serde_json::Value;
use session::{Event, EventKind, SessionId, SessionStore};
use tasks::Task;

/// 写类工具的名字（ThinAgent 只开 write/edit；apply_patch 留给日志里已有的结果）。
const WRITE_TOOLS: [&str; 3] = ["write", "edit", "apply_patch"];

/// relay 的 request id 前缀（`task-report:<taskId>:<reportId>`）。
const TASK_REPORT_PREFIX: &str = "task-report:";

/// 一张结果卡的 id（relay 的 request id）拆成任务和报告。
pub(crate) fn parse_relay_id(call_id: &str) -> Option<(&str, &str)> {
    let rest = call_id.strip_prefix(TASK_REPORT_PREFIX)?;
    let (task_id, report_id) = rest.split_once(':')?;
    if task_id.is_empty() || report_id.is_empty() {
        return None;
    }
    Some((task_id, report_id))
}

/// 从 Kind 的日志里认这张结果卡：relay 的 request id 和任务 id 都对得上才算。
pub(crate) fn relay_card(events: &[Event], call_id: &str, task_id: &str) -> Option<Value> {
    events.iter().find_map(|event| {
        let EventKind::UserMessage { message, .. } = &event.kind else {
            return None;
        };
        let MessageSource::User {
            rpc_id: Some(rpc_id),
        } = &message.source
        else {
            return None;
        };
        if rpc_id != call_id {
            return None;
        }
        let text: String = message
            .content
            .iter()
            .filter_map(ContentBlock::as_text)
            .collect();
        let body: Value = serde_json::from_str(&text).ok()?;
        if body["kind"] != "task_report" || body["task"]["id"] != task_id {
            return None;
        }
        Some(body)
    })
}

/// 任务所有 Attempt 里写类工具的改动：只认成功结果，按出现顺序去重、相对 cwd。
pub(crate) fn changed_files(data_dir: &Path, task: &Task) -> Vec<String> {
    let cwd = task.effective_cwd();
    let mut seen = HashSet::new();
    let mut changed = Vec::new();
    for attempt in task.runs.iter().flat_map(|run| run.attempts.iter()) {
        let Some(conversation) = &attempt.conversation else {
            continue;
        };
        let Ok(session_id) = SessionId::parse(conversation.as_str()) else {
            continue;
        };
        let Ok(snapshot) = SessionStore::open_read_only(data_dir, &session_id) else {
            continue;
        };
        for path in changed_paths(snapshot.events(), cwd) {
            if seen.insert(path.clone()) {
                changed.push(path);
            }
        }
    }
    changed
}

/// 从一次 Attempt 的日志里取改动路径：写类工具的成功结果才要，diffs 里每个 path 算一个。
pub(crate) fn changed_paths(events: &[Event], cwd: &Path) -> Vec<String> {
    let mut write_calls = HashSet::new();
    let mut seen = HashSet::new();
    let mut changed = Vec::new();
    for event in events {
        match &event.kind {
            EventKind::ToolCall { call_id, name, .. } if WRITE_TOOLS.contains(&name.as_str()) => {
                write_calls.insert(call_id.as_str());
            }
            EventKind::ToolResult {
                message,
                error,
                meta,
                ..
            } => {
                let Some(call_id) = message.tool_call_id.as_deref() else {
                    continue;
                };
                if !write_calls.contains(call_id) {
                    continue;
                }
                if error.is_some() || message.is_error == Some(true) {
                    continue;
                }
                let Some(diffs) = meta
                    .as_ref()
                    .and_then(|meta| meta.get("diffs"))
                    .and_then(Value::as_array)
                else {
                    continue;
                };
                for diff in diffs {
                    let Some(path) = diff.get("path").and_then(Value::as_str) else {
                        continue;
                    };
                    let display = display_path(cwd, Path::new(path));
                    if seen.insert(display.clone()) {
                        changed.push(display);
                    }
                }
            }
            _ => {}
        }
    }
    changed
}

/// 卡上固定一行：改了 N 个文件（写入范围 X）。
pub(crate) fn changed_line(count: usize, scope: &str) -> String {
    format!("改了 {count} 个文件（写入范围 {scope}）")
}

/// 写入范围在卡上的显示：相对任务 cwd；不在 cwd 里时用原路径。
pub(crate) fn write_scope_display(task: &Task) -> Option<String> {
    task.write_scope
        .as_deref()
        .map(|scope| display_path(&task.cwd, scope))
}

/// 会话内的展示路径：cwd 相对、/ 分隔；不在 cwd 内时用原路径（照 tools 的 display_path）。
fn display_path(cwd: &Path, path: &Path) -> String {
    match path.strip_prefix(cwd) {
        Ok(relative) => {
            let parts: Vec<String> = relative
                .components()
                .filter_map(|component| match component {
                    Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
                    _ => None,
                })
                .collect();
            if parts.is_empty() {
                ".".to_owned()
            } else {
                parts.join("/")
            }
        }
        Err(_) => path.to_string_lossy().into_owned(),
    }
}
