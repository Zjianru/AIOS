//! 内置 glob 工具：按修改时间排序、最多 100 条的文件发现。
//!
//! 参数、上限、输出格式和错误文字照 DSH 的 packages/fs/tool-fs-search/src/glob.ts。

use std::fs;
use std::time::SystemTime;

use globset::Glob;
use ignore::WalkBuilder;
use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFailure, ToolFuture, ToolResult};

use super::{display_path, resolve_path};

/// 单次 glob 最多显示的路径数。
pub const GLOB_MAX_RESULTS: usize = 100;

/// 永不进入的 VCS 元数据目录名（照 DSH 的 GLOB_VCS_EXCLUDES）。
const VCS_EXCLUDES: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// 内置 glob 工具。
pub struct GlobTool {
    parameters: Value,
}

impl GlobTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Glob pattern to match file paths against (e.g. \"**/*.ts\", \"src/**/*.test.js\"). A pattern with no \"/\" matches the basename at any depth, so \"*\" and \"*.ts\" both search the whole tree; include a separator to anchor the depth."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in. Defaults to the session workspace; a relative path resolves against it."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }
}

impl Default for GlobTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files, not directories, whose paths match a glob pattern, including hidden and ignored files. Returns up to 100 paths in modification-time order; a larger result keeps the first paths and reports where the complete list was saved."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn concurrency(&self) -> ToolConcurrency {
        ToolConcurrency::Parallel
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            match glob(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn glob(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let pattern = object
        .get("pattern")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("pattern must be a non-empty string"))?
        .to_owned();
    if pattern.trim().is_empty() {
        return Err(ToolFailure::plain("pattern must be a non-empty string"));
    }
    let requested_path = match object.get("path") {
        None => None,
        Some(value) => {
            let text = value
                .as_str()
                .ok_or_else(|| ToolFailure::plain("path must be a string when given"))?;
            if text.trim().is_empty() {
                return Err(ToolFailure::plain(
                    "path must be a non-empty string when given",
                ));
            }
            Some(text.to_owned())
        }
    };
    let matcher = Glob::new(&pattern)
        .map_err(|error| {
            ToolFailure::coded(
                "SEARCH_INVALID_PATTERN",
                format!("glob pattern rejected by ripgrep: {error}"),
            )
        })?
        .compile_matcher();
    let cwd = fs::canonicalize(&context.cwd).map_err(|error| {
        ToolFailure::coded(
            "SEARCH_FAILED",
            format!(
                "glob search failed: cannot access \"{}\": {error}",
                context.cwd.display()
            ),
        )
    })?;
    let root = match &requested_path {
        Some(text) => resolve_path(context, text),
        None => context.cwd.clone(),
    };
    let root = fs::canonicalize(&root).map_err(|error| {
        ToolFailure::coded(
            "SEARCH_FAILED",
            format!(
                "glob search failed: cannot access \"{}\": {error}",
                requested_path.as_deref().unwrap_or(".")
            ),
        )
    })?;

    let mut files: Vec<(String, SystemTime)> = Vec::new();
    for entry in build_walker(&root).build() {
        let Ok(entry) = entry else {
            continue;
        };
        if entry.depth() == 0 {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let path = entry.path();
        let mut display = display_path(&cwd, path);
        if requested_path.as_deref() == Some(".") && display != "." {
            display = format!("./{display}");
        }
        let matched = if pattern.contains('/') {
            matcher.is_match(display.as_str())
        } else {
            matcher.is_match(entry.file_name())
        };
        if !matched {
            continue;
        }
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        files.push((display, modified));
    }
    // 修改时间降序（最新在前）；同一时间按路径排序，保证可复现。
    files.sort_by(|(left_path, left_time), (right_path, right_time)| {
        right_time
            .cmp(left_time)
            .then_with(|| left_path.cmp(right_path))
    });
    Ok(format_paths(files))
}

fn build_walker(root: &std::path::Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .require_git(false)
        .follow_links(false);
    builder.filter_entry(|entry| {
        let name = entry.file_name().to_string_lossy();
        !VCS_EXCLUDES
            .iter()
            .any(|excluded| *excluded == name.as_ref())
    });
    builder
}

fn format_paths(files: Vec<(String, SystemTime)>) -> String {
    if files.is_empty() {
        return "No files found".to_owned();
    }
    let paths: Vec<String> = files.into_iter().map(|(path, _)| path).collect();
    if paths.len() <= GLOB_MAX_RESULTS {
        return paths.join("\n");
    }
    let shown = paths[..GLOB_MAX_RESULTS].join("\n");
    format!(
        "{shown}\n\n(Showing {GLOB_MAX_RESULTS} of {} paths. The complete result could not be saved; narrow pattern or path to see more.)",
        paths.len()
    )
}
