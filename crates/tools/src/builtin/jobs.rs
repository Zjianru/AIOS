//! 内置后台任务工具：job_output / job_list / job_kill（照 DSH jobs/tool-jobs）。

use std::time::Duration;

use serde_json::{Value, json};

use crate::jobs::{JobRead, JobStatus, MAX_WAIT_MS};
use crate::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

const DEFAULT_WAIT_MS: u64 = 30_000;

fn registry(context: &ToolContext) -> &crate::jobs::TaskRegistry {
    context.session().tasks()
}

fn parse_object(arguments: &str) -> Result<Value, ToolFailure> {
    let args: Value = serde_json::from_str(arguments)
        .map_err(|error| ToolFailure::plain(format!("arguments must be a JSON object: {error}")))?;
    if !args.is_object() {
        return Err(ToolFailure::plain("arguments must be a JSON object"));
    }
    Ok(args)
}

fn job_id<'a>(object: &'a Value, tool: &str) -> Result<&'a str, ToolFailure> {
    object
        .get("job_id")
        .and_then(Value::as_str)
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| ToolFailure::plain(format!("{tool}: job_id must be a non-empty string")))
}

/// 任务输出的模型可见文本：内存尾部（丢过就写 jobs 目录的完整流路径）。
fn body_of(read: &JobRead) -> String {
    let out = stream_text(
        &read.stdout,
        read.stdout_dropped,
        read.stdout_path.display(),
    );
    let err = stream_text(
        &read.stderr,
        read.stderr_dropped,
        read.stderr_path.display(),
    );
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
    body
}

fn stream_text(text: &str, dropped: bool, path: impl std::fmt::Display) -> String {
    if !dropped {
        return text.to_owned();
    }
    format!("{text}\n[some output was dropped from memory; full output: {path}]")
}

fn status_line(status: JobStatus) -> String {
    format!("[status: {}]", status.as_str())
}

/// job_output：读一个后台任务的输出（可选等待结算）。
pub struct JobOutputTool {
    parameters: Value,
}

impl JobOutputTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "Background job id."},
                    "wait": {"type": "boolean", "description": "Wait for settlement (default false)."},
                    "timeout_ms": {"type": "number", "description": "Wait timeout in milliseconds; capped at 600000."}
                },
                "required": ["job_id"]
            }),
        }
    }
}

impl Default for JobOutputTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for JobOutputTool {
    fn name(&self) -> &str {
        "job_output"
    }

    fn description(&self) -> &str {
        "Read a background job's output (optionally wait for it to finish)."
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
            match job_output(&call.arguments, context).await {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

async fn job_output(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args = parse_object(arguments)?;
    let id = job_id(&args, "job_output")?;
    let wait = match args.get("wait") {
        None => false,
        Some(value) => value
            .as_bool()
            .ok_or_else(|| ToolFailure::plain("job_output: wait must be a boolean"))?,
    };
    let timeout_ms = match args.get("timeout_ms") {
        None => DEFAULT_WAIT_MS,
        Some(value) => {
            let timeout = value.as_f64().ok_or_else(|| {
                ToolFailure::plain("job_output: timeout_ms must be a positive number")
            })?;
            if !timeout.is_finite() || timeout <= 0.0 {
                return Err(ToolFailure::plain(
                    "job_output: timeout_ms must be a positive number",
                ));
            }
            (timeout as u64).min(MAX_WAIT_MS)
        }
    };
    let read = if wait {
        registry(context)
            .wait_async(id, Duration::from_millis(timeout_ms))
            .await?
    } else {
        registry(context).output(id)?
    };
    Ok(format!(
        "{}\n{}",
        body_of(&read),
        status_line(read.view.status)
    ))
}

/// job_list：列出本会话的后台任务。
pub struct JobListTool {
    parameters: Value,
}

impl JobListTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({"type": "object", "properties": {}}),
        }
    }
}

impl Default for JobListTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for JobListTool {
    fn name(&self) -> &str {
        "job_list"
    }

    fn description(&self) -> &str {
        "List this session's background jobs."
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
            let jobs = registry(context).list();
            let content = if jobs.is_empty() {
                "(no background jobs)".to_owned()
            } else {
                jobs.iter()
                    .map(|job| {
                        format!(
                            "{} [{}] {} — {}",
                            job.id,
                            job.kind,
                            job.status.as_str(),
                            job.label
                        )
                    })
                    .collect::<Vec<String>>()
                    .join("\n")
            };
            ToolResult::success(call.id, content)
        })
    }
}

/// job_kill：请求取消一个后台任务。
pub struct JobKillTool {
    parameters: Value,
}

impl JobKillTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "job_id": {"type": "string", "description": "Background job id."},
                    "reason": {"type": "string", "description": "Optional reason recorded on the job."}
                },
                "required": ["job_id"]
            }),
        }
    }
}

impl Default for JobKillTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for JobKillTool {
    fn name(&self) -> &str {
        "job_kill"
    }

    fn description(&self) -> &str {
        "Request cancellation of a background job."
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
            match job_kill(&call.arguments, context).await {
                Ok(content) => ToolResult::success(call.id, content),
                Err(failure) => ToolResult::error(call.id, failure),
            }
        })
    }
}

async fn job_kill(arguments: &str, context: &ToolContext) -> Result<String, ToolFailure> {
    let args = parse_object(arguments)?;
    let id = job_id(&args, "job_kill")?;
    let reason = args
        .get("reason")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let read = registry(context)
        .terminate(id, reason, crate::jobs::KILL_GRACE)
        .await?;
    let body = body_of(&read);
    let body = if body == "(no output)" {
        String::new()
    } else {
        format!("\n{body}")
    };
    Ok(format!(
        "requested cancellation of job {id}{body}\n{}",
        status_line(read.view.status)
    ))
}
