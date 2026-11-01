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

fn truncated_path(content: &str, nth: usize) -> String {
    let marker = "[output truncated; full output: ";
    let mut start = 0;
    for _ in 0..=nth {
        let found = content[start..].find(marker).expect("有截断通知");
        start += found + marker.len();
    }
    let rest = &content[start..];
    rest[..rest.find(']').unwrap()].to_owned()
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

#[tokio::test]
async fn stale_job_output_files_do_not_pollute_new_jobs() {
    // 预置一个旧实现的输出文件：新任务不能 append 到它、也不能把它读进结果。
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let jobs_dir = data_dir.path().join("jobs").join("session-sess-1");
    std::fs::create_dir_all(&jobs_dir).unwrap();
    std::fs::write(jobs_dir.join("bash-1.out"), "STALE-OUTPUT-MUST-NOT-LEAK\n").unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");

    let result = bash(json!({"command": "printf 'hello\n'"}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello\n");
    assert!(
        !result.content.contains("STALE-OUTPUT-MUST-NOT-LEAK"),
        "旧输出污染了新 job: {}",
        result.content
    );
    let stale = std::fs::read_to_string(jobs_dir.join("bash-1.out")).unwrap();
    assert_eq!(
        stale, "STALE-OUTPUT-MUST-NOT-LEAK\n",
        "旧文件不能被新 job 改写"
    );
}

#[tokio::test]
async fn job_files_and_dirs_are_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");

    let started = bash(
        json!({"command": "sleep 30", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let jobs_dir = data_dir.path().join("jobs").join("session-sess-1");
    let dir_mode = std::fs::metadata(&jobs_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "jobs 目录应为 0700");

    // kill 会等结算，保证读线程已把 .out/.err 建好，再查文件权限。
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &context).await;
    assert!(!kill.is_error, "{}", kill.content);
    let file = jobs_dir.join(format!("{job_id}.out"));
    assert!(file.exists(), "任务输出文件应在 jobs 目录：{file:?}");
    let file_mode = std::fs::metadata(&file).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "任务输出文件应为 0600");
}

#[tokio::test]
async fn job_output_wait_timeout_does_not_suppress_completion_notification() {
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let (sender, receiver) = mpsc::channel::<String>();
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |job_id: &str, _status| {
            let _ = sender.send(job_id.to_owned());
        }));

    let started = bash(
        json!({"command": "while [ ! -f go ]; do :; done; printf 'done\n'", "run_in_background": true}),
        &context,
    )
    .await;
    assert!(!started.is_error, "{}", started.content);
    let job_id = extract_job_id(&started.content);

    // 还没完成时 wait 超时返回 running；这一步不能把任务标成「已被前台等住」。
    let waited = run(
        &JobOutputTool::new(),
        json!({"job_id": job_id, "wait": true, "timeout_ms": 100}),
        &context,
    )
    .await;
    assert!(!waited.is_error, "{}", waited.content);
    assert!(
        waited.content.contains("[status: running]"),
        "{}",
        waited.content
    );

    std::fs::write(dir.path().join("go"), "").unwrap();
    let fired = receiver.recv_timeout(StdDuration::from_secs(5));
    assert!(
        fired.is_ok(),
        "job_output 的 wait 超时后，任务完成通知被吞掉了"
    );
}

#[tokio::test]
async fn foreground_fast_command_does_not_notify() {
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let (sender, receiver) = mpsc::channel::<String>();
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |job_id: &str, _status| {
            let _ = sender.send(job_id.to_owned());
        }));

    let result = bash(json!({"command": "printf 'fast\n'"}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "fast\n");
    assert!(
        receiver
            .recv_timeout(StdDuration::from_millis(200))
            .is_err(),
        "前台已被等到结算的任务不应再发完成通知"
    );
}

#[tokio::test]
async fn promoted_timeout_job_completion_notifies() {
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let (sender, receiver) = mpsc::channel::<String>();
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |job_id: &str, _status| {
            let _ = sender.send(job_id.to_owned());
        }));

    let result = bash(
        json!({"command": "while [ ! -f go ]; do :; done; printf 'done\n'", "timeoutMs": 100}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("moved to background job bash-"),
        "{}",
        result.content
    );
    std::fs::write(dir.path().join("go"), "").unwrap();
    let fired = receiver.recv_timeout(StdDuration::from_secs(5));
    assert!(fired.is_ok(), "超时转后台的任务完成后没有通知");
}

#[tokio::test]
async fn cross_session_kill_is_rejected() {
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
    let kill_b = run(&JobKillTool::new(), json!({"job_id": job_id}), &b).await;
    assert!(
        kill_b.is_error,
        "别的会话不能停这条任务：{}",
        kill_b.content
    );
    assert!(
        kill_b.content.contains("job not found"),
        "{}",
        kill_b.content
    );
    let kill_a = run(&JobKillTool::new(), json!({"job_id": job_id}), &a).await;
    assert!(!kill_a.is_error, "{}", kill_a.content);
}

#[tokio::test]
async fn foreground_large_output_reads_only_tail() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    // 5 MiB 输出（远超前台 64,000 字节上限）。
    let result = bash(
        json!({"command": "dd if=/dev/zero bs=1048576 count=5 2>/dev/null | tr '\\0' 'a'"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("[output truncated; full output: "),
        "{:.120}",
        result.content
    );
    let path = truncated_path(&result.content, 0);
    let full = std::fs::metadata(&path).unwrap().len();
    assert_eq!(full, 5 * 1024 * 1024);
    // 模型侧只剩 64,000 字节尾部 + 通知，不是 5 MiB。
    assert!(
        result.content.len() < 70_000,
        "结果不应含全文：{} 字节",
        result.content.len()
    );
}

const SLOW_ENV: &str = "AIOS_SLOW_TESTS";

#[tokio::test]
async fn foreground_output_over_spill_cap_reports_unavailable() {
    if std::env::var_os(SLOW_ENV).is_none() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    // 70 MiB > 前台 spill 上限 64 MiB：全文路径报 (unavailable)。
    let result = bash(
        json!({"command": "dd if=/dev/zero bs=1048576 count=70 2>/dev/null | tr '\\0' 'b'"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result
            .content
            .contains("[output truncated; full output: (unavailable)]"),
        "{:.160}",
        result.content
    );
}
