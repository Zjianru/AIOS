//! 内置 read 工具：带行号和分页尾注的 UTF-8 文本读取。
//!
//! 参数、上限、输出格式和错误文字照 DSH 的 packages/fs/tool-fs/src/read*.ts。

use std::fs;
use std::path::Path;

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 单次 read 的默认与最大行数。
pub const READ_LIMIT: usize = 2000;
/// 单行的最大字符数。
pub const READ_MAX_LINE_LENGTH: usize = 2000;
/// 单次 read 选中行的最大字节数。
pub const READ_MAX_BYTES: usize = 50 * 1024;

/// 内置 read 工具。
pub struct ReadTool {
    parameters: Value,
}

impl ReadTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "file_path": {"type": "string", "description": "Path to read, resolved by the filesystem backend."},
                    "offset": {"type": "number", "description": "1-based first line to return. Defaults to 1."},
                    "limit": {"type": "number", "description": "Maximum number of lines to return. Defaults to 2000."}
                },
                "required": ["file_path"]
            }),
        }
    }
}

impl Default for ReadTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for ReadTool {
    fn name(&self) -> &str {
        "read"
    }

    fn description(&self) -> &str {
        "Read a UTF-8 text file and return line-numbered content."
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
            match read(&call.arguments, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn read(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let file_path = object
        .get("file_path")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("file_path must be a non-empty string"))?
        .to_owned();
    if file_path.trim().is_empty() {
        return Err(ToolFailure::plain("file_path must be a non-empty string"));
    }
    let offset = match object.get("offset") {
        None => 1,
        Some(value) => positive_integer(value, "offset")?,
    };
    let limit = match object.get("limit") {
        None => READ_LIMIT,
        Some(value) => positive_integer(value, "limit")?,
    };
    if limit > READ_LIMIT {
        return Err(ToolFailure::plain(format!(
            "limit must be less than or equal to {READ_LIMIT}"
        )));
    }

    let requested = Path::new(&file_path);
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        context.cwd.join(requested)
    };
    let metadata = match fs::metadata(&absolute) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(ToolFailure::coded(
                "FS_NOT_FOUND",
                format!("cannot read \"{}\": not found", absolute.display()),
            ));
        }
        Err(error) => {
            return Err(ToolFailure::coded(
                "FS_IO_ERROR",
                format!("cannot read \"{}\": {error}", absolute.display()),
            ));
        }
    };
    let canonical = fs::canonicalize(&absolute).unwrap_or_else(|_| absolute.clone());
    let display = canonical.to_string_lossy().into_owned();
    if !metadata.is_file() {
        return Err(ToolFailure::coded(
            "FS_NOT_REGULAR_FILE",
            format!("cannot read \"{display}\": not a regular file"),
        ));
    }
    let bytes = fs::read(&canonical).map_err(|error| {
        ToolFailure::coded("FS_IO_ERROR", format!("cannot read \"{display}\": {error}"))
    })?;
    let content = String::from_utf8(bytes).map_err(|_| {
        ToolFailure::coded(
            "FS_IO_ERROR",
            format!("cannot read \"{display}\": the file content is not valid UTF-8"),
        )
    })?;
    window(&content, &display, offset, limit)
}

fn positive_integer(value: &Value, name: &str) -> Result<usize, ToolFailure> {
    match value.as_u64() {
        Some(number) if number >= 1 => Ok(number as usize),
        _ => Err(ToolFailure::plain(format!(
            "{name} must be a positive integer"
        ))),
    }
}

fn window(
    content: &str,
    display: &str,
    offset: usize,
    limit: usize,
) -> Result<String, ToolFailure> {
    let mut lines: Vec<&str> = if content.is_empty() {
        Vec::new()
    } else {
        content.split('\n').collect()
    };
    if content.ends_with('\n') {
        lines.pop();
    }
    let total = lines.len();
    let mut selected: Vec<(usize, String)> = Vec::new();
    let mut output_bytes = 0usize;
    let mut truncated_by_bytes = false;
    for (index, raw_line) in lines.iter().enumerate() {
        let number = index + 1;
        if truncated_by_bytes || number < offset || selected.len() >= limit {
            continue;
        }
        let text = truncate_line(raw_line, READ_MAX_LINE_LENGTH);
        let bytes = text.len() + usize::from(!selected.is_empty());
        if output_bytes + bytes > READ_MAX_BYTES {
            truncated_by_bytes = true;
            continue;
        }
        output_bytes += bytes;
        selected.push((number, text));
    }
    if !truncated_by_bytes && offset > total && !(total == 0 && offset == 1) {
        return Err(ToolFailure::coded(
            "FS_NOT_FOUND",
            format!("offset {offset} is out of range for \"{display}\" ({total} lines)"),
        ));
    }
    let end_line = selected
        .last()
        .map_or_else(|| offset.saturating_sub(1), |(number, _)| *number);
    let footer = if truncated_by_bytes {
        format!(
            "(Output capped. Showing lines {offset}-{end_line}. Use offset={} to continue.)",
            end_line + 1
        )
    } else if end_line < total {
        format!(
            "(Showing lines {offset}-{end_line} of {total}. Use offset={} to continue.)",
            end_line + 1
        )
    } else {
        format!("(End of file - total {total} lines)")
    };
    let body = if selected.is_empty() {
        footer
    } else {
        let rendered = selected
            .iter()
            .map(|(number, text)| format!("{number}: {text}"))
            .collect::<Vec<_>>()
            .join("\n");
        format!("{rendered}\n\n{footer}")
    };
    Ok(format!(
        "<path>{display}</path>\n<type>file</type>\n<content>\n{body}\n</content>"
    ))
}

fn truncate_line(line: &str, max: usize) -> String {
    let mut characters = line.chars();
    let out: String = characters.by_ref().take(max).collect();
    if characters.next().is_some() {
        format!("{out}... (line truncated to {max} chars)")
    } else {
        out
    }
}
