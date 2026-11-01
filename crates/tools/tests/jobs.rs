//! 接缝 2：后台任务 job_output / job_list / job_kill 与超时转后台（测试清单第 4 条）。

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use tools::{
    BashTool, JobKillTool, JobListTool, JobOutputTool, Tool, ToolCall, ToolContext, ToolResult,
    ToolSessionState,
};

fn session_context(cwd: &Path, data_dir: &Path, session_id: &str) -> ToolContext {
    ToolContext::with_session(cwd, Arc::new(ToolSessionState::new(data_dir, session_id)))
}

async fn run(tool: &dyn Tool, arguments: Value, context: &ToolContext) -> ToolResult {
    tool.execute(
        ToolCall {
            id: "call_1".to_owned(),
            name: tool.name().to_owned(),
            arguments: arguments.to_string(),
        },
        context,
    )
    .await
}

async fn bash(arguments: Value, context: &ToolContext) -> ToolResult {
    run(&BashTool::new(), arguments, context).await
}

fn extract_job_id(content: &str) -> String {
    let start = content.find("bash-").expect("文本里有 bash-<id>");
    let rest = &content[start..];
    let end = rest
        .find(|character: char| !(character.is_ascii_alphanumeric() || character == '-'))
        .unwrap_or(rest.len());
    rest[..end].to_owned()
}

#[tokio::test]
async fn bash_run_in_background_returns_job_id_and_output_is_readable() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");

    let started = bash(
        json!({
            "command": "printf 'hello\n'; sleep 1; printf 'bye\n'",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    assert!(!started.is_error, "{}", started.content);
    assert!(
        started.content.contains("started background job bash-"),
        "{}",
        started.content
    );
    let job_id = extract_job_id(&started.content);

    let list = run(&JobListTool::new(), json!({}), &context).await;
    assert!(!list.is_error, "{}", list.content);
    assert!(list.content.contains(&job_id), "{}", list.content);
    assert!(list.content.contains("[bash]"), "{}", list.content);

    let output = run(
        &JobOutputTool::new(),
        json!({"job_id": job_id, "wait": true, "timeout_ms": 5000}),
        &context,
    )
    .await;
    assert!(!output.is_error, "{}", output.content);
    assert!(output.content.contains("hello"), "{}", output.content);
    assert!(output.content.contains("bye"), "{}", output.content);
    assert!(
        output.content.contains("[status: completed]"),
        "{}",
        output.content
    );

    let list = run(&JobListTool::new(), json!({}), &context).await;
    assert!(list.content.contains("completed"), "{}", list.content);

    // 落在 <data-dir>/jobs/session-<id>/ 下的完整输出文件是这条任务的。
    let jobs_dir = data_dir.path().join("jobs").join("session-sess-1");
    assert!(jobs_dir.is_dir(), "jobs 目录按会话分：{jobs_dir:?}");
    assert!(
        std::fs::read_dir(&jobs_dir).unwrap().next().is_some(),
        "至少有一个任务输出文件"
    );
}

#[tokio::test]
async fn bash_timeout_promotes_to_background_job() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");

    let result = bash(json!({"command": "sleep 30", "timeoutMs": 100}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result
            .content
            .contains("still running after 100ms; moved to background job bash-"),
        "{}",
        result.content
    );
    let job_id = extract_job_id(&result.content);

    let kill = run(
        &JobKillTool::new(),
        json!({"job_id": job_id, "reason": "test cleanup"}),
        &context,
    )
    .await;
    assert!(!kill.is_error, "{}", kill.content);
    assert!(
        kill.content.contains("requested cancellation of job"),
        "{}",
        kill.content
    );
    let list = run(&JobListTool::new(), json!({}), &context).await;
    assert!(list.content.contains("killed"), "{}", list.content);
}

#[tokio::test]
async fn job_kill_stops_running_job() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");

    let started = bash(
        json!({"command": "sleep 30", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &context).await;
    assert!(!kill.is_error, "{}", kill.content);
    assert!(
        kill.content.contains("[status: killed]"),
        "{}",
        kill.content
    );
}

#[tokio::test]
async fn jobs_are_session_scoped() {
    let data_dir = tempfile::tempdir().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let a = session_context(dir.path(), data_dir.path(), "sess-a");
    let b = session_context(dir.path(), data_dir.path(), "sess-b");

    let started = bash(
        json!({"command": "sleep 30", "run_in_background": true}),
        &a,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let list_b = run(&JobListTool::new(), json!({}), &b).await;
    assert!(!list_b.is_error);
    assert!(
        list_b.content.contains("(no background jobs)"),
        "别的会话看不到这条任务：{}",
        list_b.content
    );
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &a).await;
    assert!(!kill.is_error, "{}", kill.content);
}

const NON_UTF8_CHILD_ENV: &str = "AIOS_TEST_NON_UTF8_CHILD";

#[tokio::test]
async fn non_utf8_env_var_does_not_panic() {
    // 父进程环境里塞一个非 UTF-8 的值，bash 工具不能 panic（std::env::vars 会）。
    if std::env::var_os(NON_UTF8_CHILD_ENV).is_none() {
        use std::os::unix::ffi::OsStringExt;
        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new(&exe)
            .arg("non_utf8_env_var_does_not_panic")
            .arg("--exact")
            .arg("--nocapture")
            .env(NON_UTF8_CHILD_ENV, "1")
            .env(
                "AIOS_TEST_BAD_ENV",
                std::ffi::OsString::from_vec(b"v\xFF".to_vec()),
            )
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "子进程失败\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path(), "sess-1");
    let result = bash(json!({"command": "printf 'ok\n'"}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert!(result.content.contains("ok"));
}
