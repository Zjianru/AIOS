//! 内置 grep 工具：进程内正则内容搜索，按文件分组、最多 250 条匹配。
//!
//! 参数、上限、输出格式和错误文字照 DSH 的 packages/fs/tool-fs-search/src/grep.ts；
//! 用 ignore + globset + grep-regex + grep-searcher 在进程内跑，不起 rg 子进程。

use std::fs;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobMatcher};
use grep_regex::RegexMatcher;
use grep_searcher::{Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::WalkBuilder;
use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFailure, ToolFuture, ToolResult};

use super::{display_path, resolve_path};

/// 单次 grep 内联保留的最大匹配数。
pub const GREP_MAX_MATCHES: usize = 250;
/// 单条匹配行预览的最大字节数。
pub const GREP_MAX_LINE_BYTES: usize = 2000;

const VCS_EXCLUDES: [&str; 6] = [".git", ".svn", ".hg", ".bzr", ".jj", ".sl"];

/// 内置 grep 工具。
pub struct GrepTool {
    parameters: Value,
}

impl GrepTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "pattern": {
                        "type": "string",
                        "description": "Regular expression to search for (ripgrep syntax)."
                    },
                    "path": {
                        "type": "string",
                        "description": "File or directory to search. Defaults to the session workspace; a relative path resolves against it."
                    },
                    "include": {
                        "type": "string",
                        "description": "One glob filter for which files to search (e.g. \"*.ts\", \"*.{js,jsx}\"). Not a list; negation is not supported."
                    }
                },
                "required": ["pattern"]
            }),
        }
    }
}

impl Default for GrepTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with a ripgrep regular expression. Returns matching lines with line numbers, grouped by file. Returns up to 250 matches; a larger result reports where the complete match list was saved."
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
            match grep(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn grep(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
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
    if pattern.is_empty() {
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
    let include = match object.get("include").and_then(Value::as_str) {
        None | Some("") => None,
        Some(raw) => {
            validate_include(raw)?;
            Some(raw.to_owned())
        }
    };
    let matcher = RegexMatcher::new(&pattern).map_err(|error| {
        ToolFailure::coded(
            "SEARCH_INVALID_PATTERN",
            format!("grep pattern rejected by ripgrep: {error}"),
        )
    })?;
    let include_matcher: Option<GlobMatcher> = match &include {
        None => None,
        Some(glob) => Some(
            Glob::new(glob)
                .map_err(|error| {
                    ToolFailure::coded(
                        "SEARCH_INVALID_PATTERN",
                        format!("grep include pattern rejected by ripgrep: {error}"),
                    )
                })?
                .compile_matcher(),
        ),
    };
    let cwd = fs::canonicalize(&context.cwd).map_err(|error| {
        ToolFailure::coded(
            "SEARCH_FAILED",
            format!(
                "grep search failed: cannot access \"{}\": {error}",
                context.cwd.display()
            ),
        )
    })?;
    let root = match &requested_path {
        Some(text) => resolve_path(context, text),
        None => context.cwd.clone(),
    };
    let metadata = fs::metadata(&root).map_err(|error| {
        ToolFailure::coded(
            "SEARCH_FAILED",
            format!(
                "grep search failed: cannot access \"{}\": {error}",
                requested_path.as_deref().unwrap_or(".")
            ),
        )
    })?;
    let root = fs::canonicalize(&root).map_err(|error| {
        ToolFailure::coded(
            "SEARCH_FAILED",
            format!(
                "grep search failed: cannot access \"{}\": {error}",
                requested_path.as_deref().unwrap_or(".")
            ),
        )
    })?;

    let mut searcher = SearcherBuilder::new().build();
    let mut matches: Vec<Match> = Vec::new();
    if metadata.is_file() {
        search_file(&mut searcher, &matcher, &root, &cwd, &mut matches)?;
    } else if metadata.is_dir() {
        let mut files: Vec<PathBuf> = Vec::new();
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
            let display = display_path(&cwd, entry.path());
            if let Some(include) = &include_matcher
                && !include.is_match(display.as_str())
                && !include.is_match(entry.file_name())
            {
                continue;
            }
            files.push(entry.path().to_path_buf());
        }
        files.sort();
        for file in files {
            search_file(&mut searcher, &matcher, &file, &cwd, &mut matches)?;
        }
    } else {
        return Err(ToolFailure::coded(
            "SEARCH_FAILED",
            format!(
                "grep search failed: \"{}\" is not a regular file or directory",
                requested_path.as_deref().unwrap_or(".")
            ),
        ));
    }
    Ok(format_output(matches))
}

fn validate_include(include: &str) -> Result<(), ToolFailure> {
    if include.trim().is_empty() {
        return Err(ToolFailure::plain(
            "include must be a non-empty glob when given",
        ));
    }
    if include.starts_with('!') {
        return Err(ToolFailure::plain(
            "include must be a positive glob filter; negated patterns (\"!…\") are not supported",
        ));
    }
    let mut brace_depth = 0usize;
    for character in include.chars() {
        match character {
            '{' => brace_depth += 1,
            '}' => brace_depth = brace_depth.saturating_sub(1),
            ',' if brace_depth == 0 => {
                return Err(ToolFailure::plain(
                    "include must be one glob, not a comma-separated list (use {a,b} alternation instead)",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

fn build_walker(root: &Path) -> WalkBuilder {
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

struct Match {
    path: String,
    line_number: u64,
    line: String,
}

fn search_file(
    searcher: &mut Searcher,
    matcher: &RegexMatcher,
    path: &Path,
    cwd: &Path,
    matches: &mut Vec<Match>,
) -> Result<(), ToolFailure> {
    let display = display_path(cwd, path);
    let mut sink = CollectSink::default();
    searcher
        .search_path(matcher, path, &mut sink)
        .map_err(|error| {
            ToolFailure::coded("SEARCH_FAILED", format!("grep search failed: {error}"))
        })?;
    for (line_number, line) in sink.matches {
        matches.push(Match {
            path: display.clone(),
            line_number,
            line,
        });
    }
    Ok(())
}

#[derive(Default)]
struct CollectSink {
    matches: Vec<(u64, String)>,
}

impl Sink for CollectSink {
    type Error = std::io::Error;

    fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, Self::Error> {
        let line_number = mat.line_number().unwrap_or(0);
        let line = match std::str::from_utf8(mat.bytes()) {
            Ok(text) => text.trim_end_matches(['\n', '\r']).to_owned(),
            Err(_) => "(line is not valid UTF-8)".to_owned(),
        };
        self.matches.push((line_number, line));
        Ok(true)
    }
}

fn preview_line(line: &str, max_bytes: usize) -> String {
    if line.len() <= max_bytes {
        return line.to_owned();
    }
    let mut end = max_bytes;
    while end > 0 && !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{} (line truncated)", &line[..end])
}

fn format_output(matches: Vec<Match>) -> String {
    let seen = matches.len();
    if seen == 0 {
        return "No matches found".to_owned();
    }
    let truncated = seen > GREP_MAX_MATCHES;
    let kept: Vec<Match> = matches
        .into_iter()
        .take(GREP_MAX_MATCHES)
        .map(|matched| Match {
            path: matched.path,
            line_number: matched.line_number,
            line: preview_line(&matched.line, GREP_MAX_LINE_BYTES),
        })
        .collect();
    let mut groups: Vec<(String, Vec<(u64, String)>)> = Vec::new();
    for matched in kept {
        match groups.iter_mut().find(|(path, _)| path == &matched.path) {
            Some((_, rows)) => rows.push((matched.line_number, matched.line)),
            None => groups.push((matched.path, vec![(matched.line_number, matched.line)])),
        }
    }
    let body = groups
        .iter()
        .map(|(path, rows)| {
            let rendered = rows
                .iter()
                .map(|(line_number, line)| format!("Line {line_number}: {line}"))
                .collect::<Vec<_>>()
                .join("\n");
            format!("{path}\n{rendered}")
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let header = if truncated {
        format!("Found {GREP_MAX_MATCHES} of {seen} matches")
    } else if seen == 1 {
        "Found 1 match".to_owned()
    } else {
        format!("Found {seen} matches")
    };
    if truncated {
        format!(
            "{header}\n\n{body}\n\n(The complete result could not be saved; narrow pattern, path, or include to see more.)"
        )
    } else {
        format!("{header}\n\n{body}")
    }
}
