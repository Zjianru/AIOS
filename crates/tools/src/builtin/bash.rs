//! 内置 bash 工具：每次新起 bash -c，支持超时转后台与 run_in_background。
//!
//! 语义照 DSH packages/shell/tool-bash、bash-local、shell-env：前台等结算、超时转后台、
//! 输出截尾 + 全文路径、受管 AIOS_* 环境变量、父环境凭据 scrub；不做沙箱（D38）。

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde_json::{Value, json};

use crate::jobs::{self, JobRead, JobSpec};
use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 前台 bash 默认超时（照 DSH tool-bash 的 timeoutMs 默认 120s）。
pub const DEFAULT_TIMEOUT_MS: u64 = 120_000;
/// 前台 bash 超时上限（照 DSH tool-bash 的 maxTimeoutMs 600s）。
pub const MAX_TIMEOUT_MS: u64 = 600_000;
/// 前台结果每个流的内存上限（T005.3 契约：64,000 字节，超出留尾部 + 全文路径）。
pub const MAX_OUTPUT_BYTES: usize = 64_000;

static FOREGROUND_READ_BYTES: AtomicU64 = AtomicU64::new(0);

/// 诊断计数：前台结果渲染时从 jobs 文件读进内存的累计字节数。
/// T005.4 回归用——大输出必须只读尾部，不能把整文件读进内存。
pub fn foreground_bytes_read() -> u64 {
    FOREGROUND_READ_BYTES.load(Ordering::Relaxed)
}

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
                    "description": {"type": "string", "description": "Clear, concise description of what this command does in active voice, 5-10 words (shown in the UI). Examples: \"ls\" → \"List files in current directory\"; \"git status\" → \"Show working tree status\"; \"npm install\" → \"Install package dependencies\"."},
                    "command": {"type": "string", "description": "The bash command to execute."},
                    "timeoutMs": {"type": "number", "description": "Timeout in milliseconds. On expiry the command moves to the background as a job instead of being killed. Defaults to 120000; capped at 600000."},
                    "workdir": {"type": "string", "description": "Working directory for this command. Defaults to the session workspace; a relative path is resolved against it."},
                    "run_in_background": {"type": "boolean", "description": "Run in the background and return a job id immediately (collect with job_output, stop with job_kill). No timeout applies."}
                },
                "required": ["description", "command"]
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
            match bash(&call.arguments, &call.id, context).await {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

async fn bash(
    arguments: &str,
    call_id: &str,
    context: &ToolContext,
) -> Result<String, ToolFailure> {
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
    let description = object
        .get("description")
        .and_then(Value::as_str)
        .ok_or_else(|| ToolFailure::plain("invalid description: expected a non-empty string"))?;
    if description.trim().is_empty() {
        return Err(ToolFailure::plain(
            "invalid description: expected a non-empty string",
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
    let spec = JobSpec {
        command: command.to_owned(),
        cwd: workdir,
        env: jobs::child_env(context),
        label,
    };
    // T006.8：前台输出边收边写进会话 spill 目录（照 DSH bash-local + subprocess collect）。
    let job_id = if background {
        registry.start(spec, false)?
    } else {
        registry.start_with_spill(
            spec,
            true,
            jobs::JobSpill {
                store: context.session().spill().clone(),
                stdout_name: format!("bash-{call_id}-stdout.txt"),
                stderr_name: format!("bash-{call_id}-stderr.txt"),
            },
            MAX_OUTPUT_BYTES,
        )?
    };
    if background {
        return Ok(format!("started background job {job_id}"));
    }

    let read = registry
        .wait_async(&job_id, std::time::Duration::from_millis(timeout_ms))
        .await?;
    if read.view.status.is_terminal() {
        let rendered = render_final(&read);
        registry.remove(&job_id);
        return Ok(rendered);
    }
    registry.promote(&job_id);
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

/// 前台结算的渲染：内存只留每流 64,000 字节尾部；截断时照 DSH 追加完整输出 locator。
fn render_final(read: &JobRead) -> String {
    let stdout = rendered_stream(
        &read.stdout,
        read.stdout_dropped,
        read.stdout_spill.as_deref(),
    );
    let stderr = rendered_stream(
        &read.stderr,
        read.stderr_dropped,
        read.stderr_spill.as_deref(),
    );
    render_body(&stdout, &stderr, read.view.exit_code, read.view.signal)
}

/// 单流渲染：截断时附 DSH 的 [output truncated; full output: path]（文件在会话 spill 目录）。
fn rendered_stream(text: &str, truncated: bool, spill: Option<&Path>) -> String {
    if !truncated {
        return text.to_owned();
    }
    let path = spill.map_or_else(
        || "(unavailable)".to_owned(),
        |spill| spill.display().to_string(),
    );
    format!("{text}\n[output truncated; full output: {path}]")
}

fn render_promoted(read: &JobRead, timeout_ms: u64) -> String {
    let out = memory_stream(
        &read.stdout,
        read.stdout_dropped,
        &read.stdout_path,
        read.stdout_spill.as_deref(),
    );
    let err = memory_stream(
        &read.stderr,
        read.stderr_dropped,
        &read.stderr_path,
        read.stderr_spill.as_deref(),
    );
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

fn memory_stream(text: &str, dropped: bool, path: &Path, spill: Option<&Path>) -> String {
    if !dropped {
        return text.to_owned();
    }
    let full = spill.map_or_else(|| path.to_path_buf(), Path::to_path_buf);
    format!(
        "{text}\n[some output was dropped from memory; full output: {}]",
        full.display()
    )
}

fn render_body(stdout: &str, stderr: &str, exit_code: Option<i32>, signal: Option<i32>) -> String {
    let out = stdout.to_owned();
    let err = stderr.to_owned();
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
