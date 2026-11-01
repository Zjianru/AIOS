//! 内置 bash 工具：每次新起 bash -c，支持超时转后台与 run_in_background。
//!
//! 语义照 DSH packages/shell/tool-bash、bash-local、shell-env：前台等结算、超时转后台、
//! 输出截尾 + 全文路径、受管 AIOS_* 环境变量、父环境凭据 scrub；不做沙箱（D38）。

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::jobs::{self, JobRead, JobSpec};
use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 前台 bash 默认超时（照 DSH tool-bash 的 timeoutMs 默认 120s）。
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// 前台 bash 超时上限（照 DSH tool-bash 的 maxTimeoutMs 600s）。
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// 前台结果每个流的内存上限（T005.3 契约：64,000 字节，超出留尾部 + 全文路径）。
pub const MAX_OUTPUT_BYTES: usize = 64_000;
/// 前景结果全文 spill 的单流上限（照 DSH bash-local maxSpillBytes 64 MiB）。
pub const MAX_SPILL_BYTES: usize = 64 * 1024 * 1024;

/// 内置 bash 工具。
pub struct BashTool {
    parameters: Value,
}

impl BashTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "command": {"type": "string", "description": "The bash command to execute."},
                    "timeoutMs": {"type": "number", "description": "Timeout in milliseconds. On expiry the command moves to the background as a job instead of being killed. Defaults to 120000; capped at 600000."},
                    "workdir": {"type": "string", "description": "Working directory for this command. Defaults to the session workspace; a relative path is resolved against it."},
                    "run_in_background": {"type": "boolean", "description": "Run in the background and return a job id immediately (collect with job_output, stop with job_kill). No timeout applies."}
                },
                "required": ["command"]
            }),
        }
    }
}

impl Default for BashTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Execute a bash command (bash -c) and return its stdout/stderr. Each call runs in a fresh shell; pass workdir instead of using cd. Long output is truncated to its tail; the full output is saved to a file whose path is reported when available. An unset variable expands to an empty string, so guard variables in such paths with ${VAR:?}. A foreground command that reaches its timeout keeps running as a background job instead of being killed."
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
            match bash(&call.arguments, &call.id, context) {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

fn bash(arguments: &str, call_id: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    let Some(object) = args.as_object() else {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    };
    let command = object
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("invalid command: expected a non-empty string"))?;
    if command.trim().is_empty() {
        return Err(ToolFailure::plain(
            "invalid command: expected a non-empty string",
        ));
    }
    let workdir = object
        .get("workdir")
        .and_then(Value::as_str)
        .map(|workdir| resolve_workdir(context, workdir))
        .unwrap_or_else(|| context.cwd.clone());
    let background = match object.get("run_in_background") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| ToolFailure::plain("run_in_background must be a boolean"))?,
    };
    let timeout_ms = match object.get("timeoutMs") {
        None => DEFAULT_TIMEOUT_MS,
        Some(value) => {
            let timeout = value
                .as_f64()
                .ok_or_else(|| ToolFailure::plain("timeoutMs must be a positive number"))?;
            if !timeout.is_finite() || timeout <= 0.0 {
                return Err(ToolFailure::plain("timeoutMs must be a positive number"));
            }
            (timeout as u64).min(MAX_TIMEOUT_MS)
        }
    };

    let registry = context.session().tasks();
    let label = first_line(command, 120);
    let job_id = registry.start(JobSpec {
        command: command.to_owned(),
        cwd: workdir,
        env: jobs::child_env(context),
        label,
    })?;
    if background {
        return Ok(format!("started background job {job_id}"));
    }

    let read = registry.wait(&job_id, std::time::Duration::from_millis(timeout_ms))?;
    if read.view.status.is_terminal() {
        registry.remove(&job_id);
        return Ok(render_final(context, call_id, &read));
    }
    registry.unawait(&job_id);
    Ok(render_promoted(&read, timeout_ms))
}

fn resolve_workdir(context: &ToolContext, workdir: &str) -> PathBuf {
    let path = Path::new(workdir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.cwd.join(path)
    }
}

fn first_line(command: &str, max_chars: usize) -> String {
    let line = command.lines().next().unwrap_or("").trim();
    let characters: Vec<char> = line.chars().take(max_chars).collect();
    if characters.is_empty() {
        "(empty command)".to_owned()
    } else {
        characters.iter().collect()
    }
}

/// 前台结算的渲染：优先用 jobs 目录里的完整流（保留 T005.3 的 64,000 字节尾部 + spill 契约）。
fn render_final(context: &ToolContext, call_id: &str, read: &JobRead) -> String {
    let stdout = stream_from_job(
        context,
        call_id,
        "stdout",
        &read.stdout_path,
        &read.stdout,
        read.stdout_dropped,
    );
    let stderr = stream_from_job(
        context,
        call_id,
        "stderr",
        &read.stderr_path,
        &read.stderr,
        read.stderr_dropped,
    );
    render_body(&stdout, &stderr, read.view.exit_code, read.view.signal)
}

struct StreamOutput {
    text: String,
    truncated: bool,
    spill_path: Option<PathBuf>,
}

fn stream_from_job(
    context: &ToolContext,
    call_id: &str,
    label: &str,
    path: &Path,
    fallback: &str,
    dropped: bool,
) -> StreamOutput {
    match std::fs::read(path) {
        Ok(bytes) if bytes.len() <= MAX_OUTPUT_BYTES => StreamOutput {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            truncated: false,
            spill_path: None,
        },
        Ok(bytes) => {
            let text = String::from_utf8_lossy(tail_bytes(&bytes, MAX_OUTPUT_BYTES)).into_owned();
            let spill_path = if bytes.len() <= MAX_SPILL_BYTES {
                let full = String::from_utf8_lossy(&bytes).into_owned();
                context
                    .session()
                    .spill()
                    .save_text(&format!("bash-{call_id}-{label}"), &full)
                    .ok()
                    .map(|saved| saved.locator)
            } else {
                None
            };
            StreamOutput {
                text,
                truncated: true,
                spill_path,
            }
        }
        Err(_) => StreamOutput {
            text: fallback.to_owned(),
            truncated: dropped,
            spill_path: None,
        },
    }
}

fn render_promoted(read: &JobRead, timeout_ms: u64) -> String {
    let out = memory_stream(&read.stdout, read.stdout_dropped, &read.stdout_path);
    let err = memory_stream(&read.stderr, read.stderr_dropped, &read.stderr_path);
    let mut body = out;
    if !err.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(&err);
    }
    if !body.is_empty() && !body.ends_with('\n') {
        body.push('\n');
    }
    format!(
        "{body}[still running after {timeout_ms}ms; moved to background job {}]",
        read.view.id
    )
}

fn memory_stream(text: &str, dropped: bool, path: &Path) -> String {
    if !dropped {
        return text.to_owned();
    }
    format!(
        "{text}\n[some output was dropped from memory; full output: {}]",
        path.display()
    )
}

fn render_body(
    stdout: &StreamOutput,
    stderr: &StreamOutput,
    exit_code: Option<i32>,
    signal: Option<i32>,
) -> String {
    let out = stream_text(stdout);
    let err = stream_text(stderr);
    let mut body = out;
    if !err.is_empty() {
        if !body.is_empty() && !body.ends_with('\n') {
            body.push('\n');
        }
        body.push_str("[stderr]\n");
        body.push_str(&err);
    }
    if body.is_empty() {
        body = "(no output)".to_owned();
    }
    let markers: Vec<String> = match (signal, exit_code) {
        (Some(signal), _) => vec![format!("[killed by signal: {}]", signal_name(signal))],
        (None, Some(0)) | (None, None) => Vec::new(),
        (None, Some(code)) => vec![format!("[exit code: {code}]")],
    };
    if markers.is_empty() {
        return body;
    }
    if !body.ends_with('\n') {
        body.push('\n');
    }
    format!("{body}{}", markers.join("\n"))
}

fn stream_text(stream: &StreamOutput) -> String {
    if !stream.truncated {
        return stream.text.clone();
    }
    let path = stream.spill_path.as_ref().map_or_else(
        || "(unavailable)".to_owned(),
        |path| path.display().to_string(),
    );
    format!("{}\n[output truncated; full output: {path}]", stream.text)
}

fn tail_bytes(bytes: &[u8], max: usize) -> &[u8] {
    let mut start = bytes.len().saturating_sub(max);
    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
        start += 1;
    }
    &bytes[start..]
}

fn signal_name(signal: i32) -> String {
    match signal {
        1 => "SIGHUP".to_owned(),
        2 => "SIGINT".to_owned(),
        3 => "SIGQUIT".to_owned(),
        6 => "SIGABRT".to_owned(),
        9 => "SIGKILL".to_owned(),
        11 => "SIGSEGV".to_owned(),
        13 => "SIGPIPE".to_owned(),
        14 => "SIGALRM".to_owned(),
        15 => "SIGTERM".to_owned(),
        other => other.to_string(),
    }
}
