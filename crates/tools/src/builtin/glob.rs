//! 内置 glob 工具：按修改时间排序、最多 100 条的文件发现。
//!
//! 参数、上限、输出格式和错误文字照 DSH 的 packages/fs/tool-fs-search/src/glob.ts。

use std::fs;
use std::path::Path;
use std::time::SystemTime;

use globset::{Glob, GlobMatcher};
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
        "Find files, not directories, whose paths match a glob pattern, including hidden and ignored files. Returns up to 100 paths in modification-time order; a larger result keeps the first paths."
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

    let matches = matching_files(&root, &cwd, &matcher, &pattern, requested_path.as_deref());
    let (mut files, truncated) = collect_bounded(matches, GLOB_MAX_RESULTS);
    // 修改时间升序（ripgrep --sort=modified 是升序，最旧在前）；同一时间按路径排序，保证可复现。
    files.sort_by(|(left_path, left_time), (right_path, right_time)| {
        left_time
            .cmp(right_time)
            .then_with(|| left_path.cmp(right_path))
    });
    Ok(format_paths(files, truncated))
}

/// 走搜索根，产出匹配的文件（路径、修改时间），不收集。
fn matching_files<'a>(
    root: &'a Path,
    cwd: &'a Path,
    matcher: &'a GlobMatcher,
    pattern: &'a str,
    requested_path: Option<&'a str>,
) -> impl Iterator<Item = (String, SystemTime)> + 'a {
    build_walker(root).build().filter_map(move |entry| {
        let entry = entry.ok()?;
        if entry.depth() == 0 {
            return None;
        }
        let file_type = entry.file_type()?;
        if !file_type.is_file() {
            return None;
        }
        let path = entry.path();
        // 匹配用未加 ./ 前缀的 cwd 相对路径（照 DSH：rg 的 --glob 匹配搜索根相对路径）。
        let plain = display_path(cwd, path);
        let matched = if pattern.contains('/') {
            matcher.is_match(plain.as_str())
        } else {
            matcher.is_match(entry.file_name())
        };
        if !matched {
            return None;
        }
        // 输出照 rg：显式 path="." 时路径带 ./ 前缀。
        let display = if requested_path == Some(".") && plain != "." {
            format!("./{plain}")
        } else {
            plain
        };
        let modified = entry
            .metadata()
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .unwrap_or(SystemTime::UNIX_EPOCH);
        Some((display, modified))
    })
}

/// 有界收集：收满 limit 条后再取一条只用于判断「还有更多」，然后立刻停；不先收全再截。
fn collect_bounded<I>(entries: I, limit: usize) -> (Vec<(String, SystemTime)>, bool)
where
    I: Iterator<Item = (String, SystemTime)>,
{
    let mut files = Vec::with_capacity(limit);
    for entry in entries {
        if files.len() == limit {
            return (files, true);
        }
        files.push(entry);
    }
    (files, false)
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

fn format_paths(files: Vec<(String, SystemTime)>, truncated: bool) -> String {
    if files.is_empty() {
        return "No files found".to_owned();
    }
    let paths: Vec<String> = files.into_iter().map(|(path, _)| path).collect();
    let shown = paths.join("\n");
    if !truncated {
        return shown;
    }
    format!(
        "{shown}\n\n(Showing {GLOB_MAX_RESULTS} paths; more matches were not read. Narrow pattern or path to see more.)"
    )
}

#[cfg(test)]
mod tests {
    use super::collect_bounded;
    use std::cell::Cell;
    use std::time::SystemTime;

    #[test]
    fn bounded_collection_stops_after_limit_and_reads_one_extra() {
        let consumed = Cell::new(0);
        let entries = (0..10).map(|index| {
            consumed.set(consumed.get() + 1);
            (format!("f{index}.txt"), SystemTime::UNIX_EPOCH)
        });
        let (files, truncated) = collect_bounded(entries, 3);
        assert_eq!(files.len(), 3, "只保留前 limit 条");
        assert!(truncated, "多出第 limit+1 条就是截断");
        assert_eq!(
            consumed.get(),
            4,
            "收满后再取一条判断有更多，然后立刻停，不读剩下的 6 条"
        );
    }

    #[test]
    fn bounded_collection_is_not_truncated_at_exact_limit() {
        let entries = (0..3).map(|index| (format!("f{index}.txt"), SystemTime::UNIX_EPOCH));
        let (files, truncated) = collect_bounded(entries, 3);
        assert_eq!(files.len(), 3);
        assert!(!truncated, "正好 limit 条不算截断");
    }
}
