//! 内置工具：只读的 read / glob / grep，和写类 write / edit / apply_patch。

mod apply_patch;
mod ask_user;
mod bash;
mod canvas;
mod edit;
mod glob;
mod grep;
mod jobs;
mod read;
mod skill;
mod todo;
mod write;

pub use apply_patch::ApplyPatchTool;
pub use ask_user::AskUserQuestionTool;
pub use bash::{BashTool, DEFAULT_TIMEOUT_MS, MAX_OUTPUT_BYTES, MAX_TIMEOUT_MS};
pub use canvas::CanvasTool;
pub use edit::EditTool;
pub use glob::GlobTool;
pub use grep::GrepTool;
pub use jobs::{JobKillTool, JobListTool, JobOutputTool};
pub use read::ReadTool;
pub use skill::SkillTool;
pub use todo::TodoWriteTool;
pub use write::WriteTool;

use std::path::{Component, Path, PathBuf};

use crate::ToolContext;

/// 把模型给的路径解析成绝对路径（相对会话 cwd）。
pub(crate) fn resolve_path(context: &ToolContext, requested: &str) -> PathBuf {
    let path = Path::new(requested);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.cwd.join(path)
    }
}

/// Kind 没有工作目录时的读/搜路径：只收绝对路径，~/ 由程序展开。
/// 相对路径不按任何 cwd 兜底，直接给可读的拒绝。
pub(crate) fn absolute_path(
    context: &ToolContext,
    requested: &str,
) -> Result<PathBuf, crate::ToolFailure> {
    if !context.session().absolute_paths_only() {
        return Ok(resolve_path(context, requested));
    }
    let requested = requested.trim();
    if let Some(rest) = requested.strip_prefix("~/") {
        let home = std::env::var_os("HOME")
            .filter(|home| !home.is_empty())
            .map(PathBuf::from)
            .filter(|home| home.is_absolute())
            .ok_or_else(|| {
                crate::ToolFailure::plain("路径要写全：以 / 或 ~/ 开头（找不到 HOME，展不开 ~/）")
            })?;
        return Ok(home.join(rest));
    }
    let path = Path::new(requested);
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Err(crate::ToolFailure::coded(
        "PATH_NOT_ABSOLUTE",
        "路径要写全：以 / 或 ~/ 开头".to_owned(),
    ))
}

/// 会话内的展示路径：cwd 相对、/ 分隔；不在 cwd 内时用原路径。
pub(crate) fn display_path(cwd: &Path, path: &Path) -> String {
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

#[cfg(test)]
mod absolute_path_tests {
    use std::path::Path;
    use std::sync::Arc;

    use crate::builtin::{GlobTool, GrepTool, ReadTool};
    use crate::{Tool, ToolCall, ToolContext, ToolSessionState};

    fn call(name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: "call-1".to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        }
    }

    fn context(dir: &Path) -> ToolContext {
        let session = Arc::new(ToolSessionState::new(dir, "absolute-test"));
        session.set_absolute_paths_only(true);
        session.set_workspace_context(false);
        ToolContext::with_session(dir, session)
    }

    #[tokio::test]
    async fn kind_read_rejects_relative_paths_and_accepts_absolute() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("note.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let context = context(dir.path());
        let read = ReadTool::new();
        let relative = read
            .execute(call("read", r#"{"file_path":"note.txt"}"#), &context)
            .await;
        assert!(relative.is_error, "相对路径要拒");
        assert!(
            relative.content.contains("路径要写全：以 / 或 ~/ 开头"),
            "{}",
            relative.content
        );
        let absolute = read
            .execute(
                call("read", &serde_json::json!({"file_path": file}).to_string()),
                &context,
            )
            .await;
        assert!(!absolute.is_error, "{}", absolute.content);
        assert!(absolute.content.contains("hello"));
    }

    #[tokio::test]
    async fn kind_glob_and_grep_require_and_accept_absolute_paths() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "hit\n").unwrap();
        let context = context(dir.path());
        let relative = GlobTool::new()
            .execute(call("glob", r#"{"pattern":"*.txt"}"#), &context)
            .await;
        assert!(relative.is_error, "glob 没给路径要拒");
        assert!(relative.content.contains("路径要写全"));
        let absolute = GlobTool::new()
            .execute(
                call(
                    "glob",
                    &serde_json::json!({"pattern": "*.txt", "path": dir.path()}).to_string(),
                ),
                &context,
            )
            .await;
        assert!(!absolute.is_error, "{}", absolute.content);
        assert!(absolute.content.contains("a.txt"), "{}", absolute.content);
        let grep = GrepTool::new()
            .execute(
                call(
                    "grep",
                    &serde_json::json!({"pattern": "hit", "path": dir.path()}).to_string(),
                ),
                &context,
            )
            .await;
        assert!(!grep.is_error, "{}", grep.content);
        assert!(grep.content.contains("hit"), "{}", grep.content);
    }
}
