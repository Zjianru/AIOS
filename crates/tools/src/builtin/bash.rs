//! 内置 bash 工具：每次新起 bash -c，返回 stdout/stderr/退出码。
//!
//! 输出截断、渲染、受管环境变量照 DSH 的 packages/shell/tool-bash、bash-local、
//! shell-env；不做沙箱（D38），不提权、不宣称受限。

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde_json::{Value, json};

use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

/// 每个输出流的内存上限（照 DSH bash-local 的 maxOutputBytes 默认 64,000 字节）。
pub const MAX_OUTPUT_BYTES: usize = 64_000;
/// spill 文件的单流上限（照 DSH bash-local 的 maxSpillBytes 默认 64 MiB）。
pub const MAX_SPILL_BYTES: usize = 64 * 1024 * 1024;
/// 受管环境变量前缀（照 DSH shell-env，前缀换成 AIOS_）。
const ENV_PREFIX: &str = "AIOS_";
/// 终端友好覆盖：关颜色/分页器/交互特性（照 DSH bash-local ENV_OVERRIDES）。
const ENV_OVERRIDES: [(&str, &str); 4] = [
    ("NO_COLOR", "1"),
    ("TERM", "dumb"),
    ("PAGER", "cat"),
    ("GIT_PAGER", "cat"),
];

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
                    "workdir": {"type": "string", "description": "Working directory for this command. Defaults to the session workspace; a relative path is resolved against it."}
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
        "Execute a bash command (bash -c) and return its stdout/stderr. Each call runs in a fresh shell; pass workdir instead of using cd. Managed $AIOS_* variables expose current harness environment facts. Long output is truncated to its tail; the full output is saved to a file whose path is reported when available. An unset variable expands to an empty string, so guard variables in such paths with ${VAR:?}."
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

/// 一个输出流：模型看到的文本、是否被截断、完整内容落盘路径。
struct StreamOutput {
    text: String,
    truncated: bool,
    spill_path: Option<PathBuf>,
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

    let output = spawn(command, &workdir, context).map_err(|error| {
        ToolFailure::plain(format!("cannot run bash in {}: {error}", workdir.display()))
    })?;
    let stdout = collect_stream(context, &output.stdout, call_id, "stdout");
    let stderr = collect_stream(context, &output.stderr, call_id, "stderr");
    Ok(render_result(
        &stdout,
        &stderr,
        output.status.code(),
        output.status.signal(),
    ))
}

fn resolve_workdir(context: &ToolContext, workdir: &str) -> PathBuf {
    let path = Path::new(workdir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        context.cwd.join(path)
    }
}

/// 起一个新的 bash -c：不持久化、stdin 关闭；环境 = 继承（去掉 AIOS_*）+ 终端覆盖 + 受管变量。
fn spawn(command: &str, workdir: &Path, context: &ToolContext) -> io::Result<std::process::Output> {
    let mut process = Command::new("bash");
    process
        .arg("-c")
        .arg(command)
        .current_dir(workdir)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process.env_clear();
    for (key, value) in std::env::vars() {
        if !key.starts_with(ENV_PREFIX) {
            process.env(key, value);
        }
    }
    for (key, value) in ENV_OVERRIDES {
        process.env(key, value);
    }
    for (key, value) in managed_env(context) {
        process.env(key, value);
    }
    process.output()
}

/// 受管 AIOS_* 变量（照 DSH shell-env 的 built-ins，挑本任务需要的）：
/// AIOS_SHELL=1、AIOS_SESSION_ID=<会话 id>、AIOS_HOME=<data-dir>。
fn managed_env(context: &ToolContext) -> Vec<(String, String)> {
    let mut variables = vec![("AIOS_SHELL".to_owned(), "1".to_owned())];
    if let Some(session_id) = context.session().session_id() {
        variables.push(("AIOS_SESSION_ID".to_owned(), session_id.to_owned()));
    }
    if let Some(data_dir) = context.session().data_dir() {
        variables.push(("AIOS_HOME".to_owned(), data_dir.display().to_string()));
    }
    variables
}

/// 收一个流：超过上限只留尾部，完整内容 spill 到会话目录并把路径写进通知。
fn collect_stream(context: &ToolContext, bytes: &[u8], call_id: &str, label: &str) -> StreamOutput {
    if bytes.len() <= MAX_OUTPUT_BYTES {
        return StreamOutput {
            text: String::from_utf8_lossy(bytes).into_owned(),
            truncated: false,
            spill_path: None,
        };
    }
    let text = String::from_utf8_lossy(tail_bytes(bytes, MAX_OUTPUT_BYTES)).into_owned();
    let spill_path = if bytes.len() <= MAX_SPILL_BYTES {
        let full = String::from_utf8_lossy(bytes).into_owned();
        context
            .session()
            .spill()
            .save_text(&format!("bash-{call_id}-{label}"), &full)
            .ok()
            .map(|saved| saved.locator)
    } else {
        // 超过 spill 上限（DSH 同语义）：只在内存里留尾部，路径报 (unavailable)。
        None
    };
    StreamOutput {
        text,
        truncated: true,
        spill_path,
    }
}

/// 取字节串的尾部 max 字节，起点对齐 UTF-8 字符边界。
fn tail_bytes(bytes: &[u8], max: usize) -> &[u8] {
    let mut start = bytes.len().saturating_sub(max);
    while start < bytes.len() && (bytes[start] & 0xC0) == 0x80 {
        start += 1;
    }
    &bytes[start..]
}

/// 照 DSH bash-local/tool-bash 的 renderResult：stdout、[stderr] 段、退出状态 marker。
fn render_result(
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

/// 信号名；常见信号用 DSH/Node 的写法，其余退回编号。
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

#[cfg(unix)]
trait ExitSignal {
    fn signal(&self) -> Option<i32>;
}

#[cfg(unix)]
impl ExitSignal for std::process::ExitStatus {
    fn signal(&self) -> Option<i32> {
        use std::os::unix::process::ExitStatusExt;
        ExitStatusExt::signal(self)
    }
}

#[cfg(not(unix))]
trait ExitSignal {
    fn signal(&self) -> Option<i32>;
}

#[cfg(not(unix))]
impl ExitSignal for std::process::ExitStatus {
    fn signal(&self) -> Option<i32> {
        None
    }
}
