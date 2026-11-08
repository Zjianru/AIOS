//! 接缝 2：后台任务 job_output / job_list / job_kill 与超时转后台（测试清单第 4 条）。

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use tools::{
    BashTool, JobKillTool, JobListTool, JobOutputTool, JobSpec, Tool, ToolCall, ToolContext,
    ToolResult, ToolSessionState,
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
            "description": "test command", "command": "printf 'hello\n'; sleep 1; printf 'bye\n'",
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
        output.content.contains("[status: completed, exit code: 0]"),
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

    let result = bash(
        json!({"description": "test command", "command": "sleep 30", "timeoutMs": 100}),
        &context,
    )
    .await;
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
        json!({"description": "test command", "command": "sleep 30", "run_in_background": true}),
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
        json!({"description": "test command", "command": "sleep 30", "run_in_background": true}),
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
    let result = bash(
        json!({"description": "test command", "command": "printf 'ok\n'"}),
        &context,
    )
    .await;
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

    let result = bash(
        json!({"description": "test command", "command": "printf 'hello\n'"}),
        &context,
    )
    .await;
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
        json!({"description": "test command", "command": "sleep 30", "run_in_background": true}),
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
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = sender.send(settled.id.clone());
        },
    ));

    let started = bash(
        json!({"description": "test command", "command": "while [ ! -f go ]; do :; done; printf 'done\n'", "run_in_background": true}),
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
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = sender.send(settled.id.clone());
        },
    ));

    let result = bash(
        json!({"description": "test command", "command": "printf 'fast\n'"}),
        &context,
    )
    .await;
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
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = sender.send(settled.id.clone());
        },
    ));

    let result = bash(
        json!({"description": "test command", "command": "while [ ! -f go ]; do :; done; printf 'done\n'", "timeoutMs": 100}),
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
        json!({"description": "test command", "command": "sleep 30", "run_in_background": true}),
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
    let bytes_before = tools::foreground_bytes_read();
    let result = bash(
        json!({"description": "test command", "command": "dd if=/dev/zero bs=1048576 count=5 2>/dev/null | tr '\\0' 'a'"}),
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
    // 可观测计数：渲染时读进内存的字节数只应约等于尾部，不能是整文件。
    let read_delta = tools::foreground_bytes_read() - bytes_before;
    assert!(
        read_delta <= 70_000,
        "前台渲染读进内存 {read_delta} 字节（应只读尾部 64,000）"
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
        json!({"description": "test command", "command": "dd if=/dev/zero bs=1048576 count=70 2>/dev/null | tr '\\0' 'b'"}),
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

/// 轮询到任务结算（受控 gate 用；不靠 sleep，靠 yield + 截止时间兜底）。
async fn wait_until_terminal(context: &ToolContext, job_id: &str) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let read = context.session().tasks().output(job_id).expect("任务存在");
        if read.view.status.is_terminal() {
            return;
        }
        assert!(std::time::Instant::now() < deadline, "任务没有在 5s 内结算");
        tokio::task::yield_now().await;
    }
}

#[tokio::test]
async fn foreground_fast_settlement_without_wait_does_not_notify() {
    // 旧语义：awaited 只在 wait 里置位，所以 start 后立刻结算会先发完成通知。
    // 新语义：start(foreground) 时就标记 awaited，不调用 wait 也不发通知。
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let (sender, receiver) = mpsc::channel::<String>();
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = sender.send(settled.id.clone());
        },
    ));

    let job_id = context
        .session()
        .tasks()
        .start(
            JobSpec {
                command: "printf 'hi\n'".to_owned(),
                cwd: dir.path().to_path_buf(),
                env: Vec::new(),
                label: "fast".to_owned(),
                tool_call: None,
            },
            true,
        )
        .unwrap();
    // 不调用 wait：直接等结算。旧实现会在这里发通知。
    wait_until_terminal(&context, &job_id).await;
    assert!(
        receiver
            .recv_timeout(StdDuration::from_millis(300))
            .is_err(),
        "前台 start 后结算不应发完成通知"
    );
}

#[tokio::test]
async fn promote_after_settlement_still_notifies() {
    // 构造 fix-now 1 的竞态：wait 超时返回 running 之后、promote 之前任务已结算。
    // 旧语义：wait 置 awaited=true，unawait 只清零不补发 → 通知永久丢。
    // T006.9：结算照旧写生命周期回调一次；promote 只补完成通知，不重复结算。
    use std::sync::Mutex;
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let (sender, receiver) = mpsc::channel::<String>();
    let settled_records = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let settled_sink = std::sync::Arc::clone(&settled_records);
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |settled: &tools::JobSettled| {
            settled_sink
                .lock()
                .expect("结算记录锁被毒化")
                .push(settled.id.clone());
        }));
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = sender.send(settled.id.clone());
        },
    ));

    let job_id = context
        .session()
        .tasks()
        .start(
            JobSpec {
                command: "while [ ! -f go ]; do :; done; printf 'done\n'".to_owned(),
                cwd: dir.path().to_path_buf(),
                env: Vec::new(),
                label: "gated".to_owned(),
                tool_call: None,
            },
            true,
        )
        .unwrap();
    let waited = context
        .session()
        .tasks()
        .wait(&job_id, StdDuration::from_millis(50))
        .unwrap();
    assert!(!waited.view.status.is_terminal(), "gate 没起作用");
    std::fs::write(dir.path().join("go"), "").unwrap();
    wait_until_terminal(&context, &job_id).await;
    // 结算发生在 promote 之前：promote 必须补发一次通知。
    context.session().tasks().promote(&job_id);
    let fired = receiver.recv_timeout(StdDuration::from_secs(1));
    assert!(fired.is_ok(), "结算与 promote 之间的完成通知被吞掉了");
    assert_eq!(
        settled_records.lock().expect("结算记录锁被毒化").len(),
        1,
        "promote 只补通知，不重复写结算生命周期记录"
    );
}

#[tokio::test]
async fn existing_jobs_dir_permissions_are_fixed_on_start() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let jobs_dir = data_dir.path().join("jobs").join("session-sess-1");
    std::fs::create_dir_all(&jobs_dir).unwrap();
    std::fs::set_permissions(&jobs_dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    let old_file = jobs_dir.join("bash-old.out");
    std::fs::write(&old_file, "old\n").unwrap();
    std::fs::set_permissions(&old_file, std::fs::Permissions::from_mode(0o644)).unwrap();

    // 建会话（ToolSessionState::new）应把旧目录/旧文件权限修正。
    let _context = session_context(dir.path(), data_dir.path(), "sess-1");
    let dir_mode = std::fs::metadata(&jobs_dir).unwrap().permissions().mode() & 0o777;
    assert_eq!(dir_mode, 0o700, "已存在的 jobs 目录应被 chmod 0700");
    let file_mode = std::fs::metadata(&old_file).unwrap().permissions().mode() & 0o777;
    assert_eq!(file_mode, 0o600, "旧输出文件应被 chmod 0600（保留归档）");
}

/// T006.9b 照 DSH tool-jobs 的 killedByModel：模型自己经 job_kill 请求取消（requested）时
/// 打 killedByModel 标记；抑制完成通知的判定在通知投递方（agent::jobs 的集成测试），
/// 这里测注册表层的 requested 判定与标记；job/settled 生命周期记录照写（status=killed/cause=kill）。
#[tokio::test]
async fn job_kill_marks_killed_by_model_for_requested_kill() {
    use std::sync::Mutex;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let settled = std::sync::Arc::new(Mutex::new(Vec::<tools::JobSettled>::new()));
    let settled_sink = std::sync::Arc::clone(&settled);
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |record: &tools::JobSettled| {
            settled_sink
                .lock()
                .expect("结算记录锁被毒化")
                .push(record.clone());
        }));
    let started = bash(
        json!({"description": "long", "command": "sleep 30", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &context).await;
    assert!(!kill.is_error, "{}", kill.content);
    assert!(
        kill.content.contains("requested cancellation of job"),
        "{}",
        kill.content
    );
    wait_until(|| !settled.lock().expect("结算记录锁被毒化").is_empty()).await;
    let record = settled.lock().expect("结算记录锁被毒化")[0].clone();
    assert_eq!(record.status, tools::JobStatus::Killed);
    assert_eq!(record.cause, tools::JobCause::Kill);
    assert!(
        context.session().tasks().take_killed_by_model(&job_id),
        "requested kill 要打 killedByModel 标记（DSH tool-jobs 在 result=requested 时 add）"
    );
    assert!(
        !context.session().tasks().take_killed_by_model(&job_id),
        "标记取走一次就清（DSH 在结算事件处理时 delete）"
    );
}

/// DSH 的 already-finished：任务已经结算时 kill 返回 already-finished，不打 killedByModel。
#[tokio::test]
async fn job_kill_of_finished_job_does_not_mark_killed_by_model() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let started = bash(
        json!({"description": "fast", "command": "printf 'fast\n'", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    wait_until_terminal(&context, &job_id).await;
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &context).await;
    assert!(!kill.is_error, "{}", kill.content);
    assert!(
        !context.session().tasks().take_killed_by_model(&job_id),
        "已结束的任务不算模型 kill，不打标记"
    );
}

/// T006.9b fix-now：request_kill(by_model=true) 在同一个 state 锁内置 kill_requested 与
/// killedByModel 标记；任务在 kill 请求后**立刻自然完成**（不等 SIGTERM 生效）时，结算回调
/// 也必须看得到标记（没有“两把锁之间”的窗口），通知投递方才能抑制重复通知。
#[tokio::test]
async fn request_kill_marks_killed_by_model_atomically_on_fast_natural_finish() {
    use std::sync::Mutex;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let shared = context.shared_session();
    let records = std::sync::Arc::new(Mutex::new(
        Vec::<(tools::JobStatus, tools::JobCause, bool)>::new(),
    ));
    let sink = std::sync::Arc::clone(&records);
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |record: &tools::JobSettled| {
            // 模拟通知投递方：结算回调里立刻 take 标记（DSH 在 settled 事件处理时 delete）。
            let suppressed = shared.tasks().take_killed_by_model(&record.id);
            sink.lock()
                .expect("竞态记录锁被毒化")
                .push((record.status, record.cause, suppressed));
        }));
    let started = bash(
        json!({
            "description": "gated natural finish",
            "command": "trap '' TERM; printf 'ready\\n'; while [ ! -f go ]; do :; done; printf 'done'",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    // 先等 trap 装好（否则 SIGTERM 可能先到、bash 被信号杀掉而非自然完成）。
    let registry = context.session().tasks();
    wait_until(|| {
        registry
            .output(&job_id)
            .is_ok_and(|read| read.stdout.contains("ready"))
    })
    .await;
    let requested = registry.request_kill(&job_id, None, true).unwrap();
    assert!(requested, "任务在跑时应是 requested");
    // kill 请求之后立刻自然完成：命令自己看到 go 后 exit 0（不依赖 SIGTERM 生效）。
    std::fs::write(dir.path().join("go"), "").unwrap();
    let read = context
        .session()
        .tasks()
        .await_termination(&job_id, std::time::Duration::from_secs(3))
        .await
        .unwrap();
    assert_eq!(
        read.view.status,
        tools::JobStatus::Completed,
        "自然完成照 producer 事实：{read:?}"
    );
    wait_until(|| !records.lock().expect("竞态记录锁被毒化").is_empty()).await;
    let (status, cause, suppressed) = records.lock().expect("竞态记录锁被毒化")[0];
    assert_eq!(status, tools::JobStatus::Completed);
    assert_eq!(
        cause,
        tools::JobCause::Kill,
        "kill 请求后自然完成，DSH settleCause 仍是 kill"
    );
    assert!(
        suppressed,
        "标记必须在结算回调可见前就位（同一 state 锁内原子置位）"
    );
}

/// T006.9b：非模型发起的 kill（直接走注册表 terminate，模拟用户/操作方取消）照旧投完成通知；
/// DSH 只对 tool-jobs 的 killedByModel 标记抑制。
#[tokio::test]
async fn operator_kill_still_notifies() {
    use std::sync::mpsc;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let (sender, receiver) = mpsc::channel::<String>();
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = sender.send(settled.id.clone());
        },
    ));
    let started = bash(
        json!({"description": "long", "command": "sleep 30", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let read = context
        .session()
        .tasks()
        .terminate(
            &job_id,
            Some("user stop".to_owned()),
            std::time::Duration::from_secs(3),
        )
        .await
        .unwrap();
    assert_eq!(read.view.status, tools::JobStatus::Killed, "{read:?}");
    let fired = receiver.recv_timeout(StdDuration::from_secs(5));
    assert_eq!(
        fired.ok().as_deref(),
        Some(job_id.as_str()),
        "非模型 kill 的结算仍应投完成通知"
    );
}

#[tokio::test]
async fn job_kill_uses_sigterm_grace_then_sigkill() {
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    // 忽略 SIGTERM 的进程组：grace 期间必须先收到 SIGTERM、输出继续被收，3s 后才 SIGKILL。
    let started = bash(
        json!({
            "description": "ignore term",
            "command": "trap '' TERM; printf 'ready\n'; sleep 10 & wait",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let registry = context.session().tasks();
    let mut ready = false;
    for _ in 0..100 {
        if registry.output(&job_id).unwrap().stdout.contains("ready") {
            ready = true;
            break;
        }
        // T009.H6：等后台子进程输出 ready；轮询间隔
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready, "先等子进程输出 ready");
    let began = Instant::now();
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &context).await;
    let elapsed = began.elapsed();
    assert!(!kill.is_error, "{}", kill.content);
    assert!(
        elapsed >= Duration::from_millis(2_500),
        "SIGTERM 后应等 grace（约 3s）再 SIGKILL，实际 {elapsed:?}"
    );
    let read = registry.output(&job_id).unwrap();
    assert_eq!(read.view.status, tools::JobStatus::Killed, "{read:?}");
    assert_eq!(
        read.view.signal,
        Some(9),
        "应记录 SIGKILL 终止信号：{read:?}"
    );
    assert!(
        read.stdout.contains("ready"),
        "grace 期间输出仍要收：{}",
        read.stdout
    );
}

#[tokio::test]
async fn job_kill_grace_lets_sigterm_handler_run() {
    use std::time::{Duration, Instant};

    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let started = bash(
        json!({
            "description": "handle term",
            "command": "trap 'echo got-term; exit 0' TERM; printf 'ready\n'; sleep 30",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let registry = context.session().tasks();
    for _ in 0..100 {
        if registry.output(&job_id).unwrap().stdout.contains("ready") {
            break;
        }
        // T009.H6：等后台子进程输出 ready；轮询间隔
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let began = Instant::now();
    let kill = run(&JobKillTool::new(), json!({"job_id": job_id}), &context).await;
    let elapsed = began.elapsed();
    assert!(!kill.is_error, "{}", kill.content);
    assert!(
        elapsed < Duration::from_millis(2_500),
        "SIGTERM handler 正常退出，不该等满 grace：{elapsed:?}"
    );
    // 照 DSH shell/tool-bash 的 processOutcome：SIGTERM handler 自己 exit 0 就是 completed
    // （kill 没落地，detail 记 producer 的 exit code）；grace 期间输出照样收。
    let read = registry.output(&job_id).unwrap();
    assert_eq!(read.view.status, tools::JobStatus::Completed, "{read:?}");
    assert_eq!(read.view.detail.as_deref(), Some("exit code: 0"));
    assert!(
        read.stdout.contains("got-term"),
        "SIGTERM handler 的 grace 输出应被收：{}",
        read.stdout
    );
}

#[tokio::test]
async fn job_id_matches_dsh_kind_short_uuid() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let started = bash(
        json!({"description": "id check", "command": "printf x", "run_in_background": true}),
        &context,
    )
    .await;
    let id = extract_job_id(&started.content);
    let suffix = id.strip_prefix("bash-").expect("bash- 前缀（kind）");
    assert_eq!(suffix.len(), 16, "DSH 用 UUID 的 16 位小写十六进制：{id}");
    assert!(
        suffix
            .chars()
            .all(|character| character.is_ascii_digit() || ('a'..='f').contains(&character)),
        "{id}"
    );
    let _ = run(&JobKillTool::new(), json!({"job_id": id}), &context).await;
}

#[tokio::test]
async fn job_output_reads_model_cursor_once() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let started = bash(
        json!({
            "description": "two bursts",
            "command": "printf 'first\n'; while [ ! -f go ]; do sleep 0.01; done; printf 'second\n'",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let mut first = String::new();
    for _ in 0..100 {
        let out = run(&JobOutputTool::new(), json!({"job_id": job_id}), &context).await;
        if out.content.contains("first") {
            first = out.content;
            break;
        }
        // T009.H6：等后台子进程输出 first；轮询间隔
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(first.contains("first"), "第一次游标读拿到 first：{first}");
    std::fs::write(dir.path().join("go"), "go").unwrap();
    let out = run(
        &JobOutputTool::new(),
        json!({"job_id": job_id, "wait": true, "timeout_ms": 5000}),
        &context,
    )
    .await;
    assert!(
        out.content.contains("second"),
        "续读拿到 second：{}",
        out.content
    );
    assert!(
        !out.content.contains("first"),
        "游标读不重复返回旧输出：{}",
        out.content
    );
    let out = run(&JobOutputTool::new(), json!({"job_id": job_id}), &context).await;
    assert!(
        out.content.contains("(no new output)"),
        "再读没有新输出：{}",
        out.content
    );
}

/// 轮询直到条件成立（受控时序用；不靠固定 sleep）。
async fn wait_until(mut predicate: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while !predicate() {
        assert!(std::time::Instant::now() < deadline, "条件没有在 5s 内成立");
        tokio::task::yield_now().await;
    }
}

/// T006.9 照 DSH jobs-local：注册回调带 jobId/owner/kind/label/startedAt/toolCall，
/// 前台命令（有 registry 时也注册）同样记一条。
#[tokio::test]
async fn registered_callback_reports_dsh_registration() {
    use std::sync::Mutex;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let records = std::sync::Arc::new(Mutex::new(Vec::<tools::JobRegistered>::new()));
    let sink = std::sync::Arc::clone(&records);
    context
        .session()
        .tasks()
        .set_on_registered(std::sync::Arc::new(
            move |registered: &tools::JobRegistered| {
                sink.lock()
                    .expect("注册记录锁被毒化")
                    .push(registered.clone());
            },
        ));

    let started = bash(
        json!({"description": "id check", "command": "printf x", "run_in_background": true}),
        &context,
    )
    .await;
    assert!(!started.is_error, "{}", started.content);
    let job_id = extract_job_id(&started.content);
    wait_until(|| records.lock().expect("注册记录锁被毒化").len() == 1).await;
    let record = {
        let records = records.lock().expect("注册记录锁被毒化");
        records[0].clone()
    };
    assert_eq!(record.id, job_id);
    assert_eq!(record.owner.as_deref(), Some("sess-1"), "owner 是会话 id");
    assert_eq!(record.kind, "bash");
    assert_eq!(record.label, "printf x", "label 是命令首行");
    assert!(record.started_at > 0);
    let tool_call = record
        .tool_call
        .as_ref()
        .expect("DSH job/registered 的 toolCall");
    assert_eq!(tool_call.call_id, "call_1");
    assert_eq!(tool_call.root_call_id, "call_1");

    // 前台命令在 registry 组合下也注册（照 DSH tool-bash：每次调用都登记）。
    let foreground = bash(
        json!({"description": "fg check", "command": "printf y"}),
        &context,
    )
    .await;
    assert!(!foreground.is_error, "{}", foreground.content);
    wait_until(|| records.lock().expect("注册记录锁被毒化").len() == 2).await;
}

/// T006.9 照 DSH jobs-local：结算回调每次结算都记一条（前台被等到的结算也记），
/// 完成通知只发给没有被调用方收走的结算。
#[tokio::test]
async fn settled_callback_records_foreground_settlement_without_notice() {
    use std::sync::Mutex;
    use std::time::Duration as StdDuration;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let records = std::sync::Arc::new(Mutex::new(Vec::<tools::JobSettled>::new()));
    let sink = std::sync::Arc::clone(&records);
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |settled: &tools::JobSettled| {
            sink.lock().expect("结算记录锁被毒化").push(settled.clone());
        }));
    let (notified_sender, notified_receiver) = std::sync::mpsc::channel::<String>();
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            let _ = notified_sender.send(settled.id.clone());
        },
    ));

    let result = bash(
        json!({"description": "test command", "command": "printf 'fast\n'"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "fast\n");
    wait_until(|| records.lock().expect("结算记录锁被毒化").len() == 1).await;
    let settled = { records.lock().expect("结算记录锁被毒化")[0].clone() };
    assert_eq!(settled.status, tools::JobStatus::Completed);
    assert_eq!(settled.cause, tools::JobCause::Producer);
    assert!(settled.producer_reported, "bash 进程退出就是 producer 报告");
    assert_eq!(
        settled.detail.as_deref(),
        Some("exit code: 0"),
        "DSH processOutcome 的 completed detail"
    );
    assert!(
        notified_receiver
            .recv_timeout(StdDuration::from_millis(200))
            .is_err(),
        "前台已被等到的结算不发完成通知"
    );
}

/// T006.9：后台结算按「注册 → 结算 → 通知」各一次，顺序由同一条回调链保证。
#[tokio::test]
async fn background_settlement_records_then_notifies_in_order() {
    use std::sync::Mutex;
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let events = std::sync::Arc::new(Mutex::new(Vec::<String>::new()));
    let registered_sink = std::sync::Arc::clone(&events);
    context
        .session()
        .tasks()
        .set_on_registered(std::sync::Arc::new(
            move |registered: &tools::JobRegistered| {
                registered_sink
                    .lock()
                    .expect("生命周期锁被毒化")
                    .push(format!("registered:{}", registered.id));
            },
        ));
    let settled_sink = std::sync::Arc::clone(&events);
    context
        .session()
        .tasks()
        .set_on_settled(std::sync::Arc::new(move |settled: &tools::JobSettled| {
            settled_sink.lock().expect("生命周期锁被毒化").push(format!(
                "settled:{}:{}",
                settled.id,
                settled.status.as_str()
            ));
        }));
    let notify_sink = std::sync::Arc::clone(&events);
    context.session().tasks().set_on_notify(std::sync::Arc::new(
        move |settled: &tools::JobSettled| {
            notify_sink
                .lock()
                .expect("生命周期锁被毒化")
                .push(format!("notify:{}", settled.id));
        },
    ));

    let started = bash(
        json!({"description": "quick", "command": "printf 'bye\n'", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    wait_until(|| events.lock().expect("生命周期锁被毒化").len() == 3).await;
    let events = events.lock().expect("生命周期锁被毒化").clone();
    assert_eq!(
        events,
        vec![
            format!("registered:{job_id}"),
            format!("settled:{job_id}:completed"),
            format!("notify:{job_id}"),
        ],
        "注册/结算/通知各一次且按序"
    );
}

/// T006.9：job_output 按 DSH renderModelDelta 渲染 stderr 段，状态行带 detail。
#[tokio::test]
async fn job_output_renders_stderr_section_and_status_detail() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let started = bash(
        json!({
            "description": "two streams",
            "command": "printf 'out'; printf 'err' >&2",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let output = run(
        &JobOutputTool::new(),
        json!({"job_id": job_id, "wait": true, "timeout_ms": 5000}),
        &context,
    )
    .await;
    assert!(!output.is_error, "{}", output.content);
    assert_eq!(
        output.content, "out\n[stderr]\nerr\n[status: completed, exit code: 0]",
        "stdout 后一个 [stderr] 段（DSH renderModelDelta）"
    );
}

/// T006.9：bash 非零退出照 DSH processOutcome 仍是 completed，退出码进 detail。
#[tokio::test]
async fn non_zero_exit_is_completed_with_exit_code_detail() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-1");
    let started = bash(
        json!({"description": "fails", "command": "exit 3", "run_in_background": true}),
        &context,
    )
    .await;
    let job_id = extract_job_id(&started.content);
    let output = run(
        &JobOutputTool::new(),
        json!({"job_id": job_id, "wait": true, "timeout_ms": 5000}),
        &context,
    )
    .await;
    assert!(!output.is_error, "{}", output.content);
    assert!(
        output.content.contains("[status: completed, exit code: 3]"),
        "DSH 把命令退出码记在 completed 的 detail 里：{}",
        output.content
    );
}

/// T006.9c 红：注册要在工具执行内同步进入待提交队列（DSH registry.start 同步 append 的落点），
/// 供 driver 在对应 tool/result 之前落 job/registered。
#[tokio::test]
async fn registration_is_queued_synchronously_for_commit() {
    let dir = tempfile::tempdir().unwrap();
    let cwd = tempfile::tempdir().unwrap();
    let state = Arc::new(ToolSessionState::new(dir.path(), "sess-9c"));
    let context = ToolContext::with_session(cwd.path(), Arc::clone(&state));
    let started = bash(
        json!({"description": "queue check", "command": "printf q", "run_in_background": true}),
        &context,
    )
    .await;
    assert!(!started.is_error, "{}", started.content);
    let pending = state.tasks().take_registered_pending();
    assert_eq!(
        pending.len(),
        1,
        "注册要同步入队（DSH registry.start 同步 append）"
    );
    assert_eq!(
        pending[0]
            .tool_call
            .as_ref()
            .expect("registration 带 toolCall")
            .call_id,
        "call_1"
    );
}

/// 后台任务输出有界：运行中单流 ≤ RETAIN_BYTES，结算后终态读把内存裁到结算保留量，
/// 淘汰时任务条目和两个输出文件一起回收（DSH jobs-local 的 settledRetainBytes 语义）。
#[tokio::test]
async fn settled_output_is_trimmed_and_removal_reclaims_job_files() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path(), "sess-retain");
    // stdout 与 stderr 各 400 KiB，都超过 RETAIN_BYTES（262144）。
    let started = bash(
        json!({
            "description": "large output",
            "command": "head -c 409600 /dev/zero | tr '\\0' x; head -c 409600 /dev/zero | tr '\\0' y >&2",
            "run_in_background": true
        }),
        &context,
    )
    .await;
    assert!(!started.is_error, "{}", started.content);
    let job_id = extract_job_id(&started.content);
    let registry = context.session().tasks();
    let settled = registry
        .wait_async(&job_id, Duration::from_secs(10))
        .await
        .expect("job 要能结算");
    assert!(
        settled.view.status.is_terminal(),
        "10 秒内应结算：{:?}",
        settled.view.status
    );

    // 结算后、模型读之前：活上限仍然生效，单流不超过 256 KiB，多出的标记 dropped。
    let before_read = registry.output(&job_id).unwrap();
    assert!(
        before_read.stdout_dropped,
        "超过 256 KiB 的 stdout 应标 dropped"
    );
    assert!(
        before_read.stderr_dropped,
        "超过 256 KiB 的 stderr 应标 dropped"
    );
    assert!(
        before_read.stdout.len() <= 262_144,
        "单流内存不得超过 RETAIN_BYTES：{}",
        before_read.stdout.len()
    );
    assert!(
        before_read.stderr.len() <= 262_144,
        "单流内存不得超过 RETAIN_BYTES：{}",
        before_read.stderr.len()
    );

    // 终态读把模型游标推到末尾，此后裁到结算保留量（DSH 默认 16384）。
    let delta = registry.read_output(&job_id).expect("终态读");
    assert!(!delta.stdout.is_empty(), "第一次读应拿到输出");
    let after_read = registry.output(&job_id).unwrap();
    assert!(
        after_read.stdout.len() <= 16 * 1024,
        "结算后读过的 stdout 应裁到 16 KiB：{}",
        after_read.stdout.len()
    );
    assert!(
        after_read.stderr.len() <= 16 * 1024,
        "结算后读过的 stderr 应裁到 16 KiB：{}",
        after_read.stderr.len()
    );
    assert!(
        after_read.stdout.len() < before_read.stdout.len(),
        "结算 + 终态读之后内存字节数应下降"
    );

    // 淘汰：任务条目和两个输出文件都回收。
    let stdout_path = after_read.stdout_path.clone();
    let stderr_path = after_read.stderr_path.clone();
    assert!(
        stdout_path.exists(),
        "淘汰前完整输出文件在：{stdout_path:?}"
    );
    assert!(
        stderr_path.exists(),
        "淘汰前完整输出文件在：{stderr_path:?}"
    );
    registry.remove(&job_id);
    assert!(registry.list().is_empty(), "淘汰后任务条目应消失");
    assert!(!stdout_path.exists(), "淘汰后 stdout 文件应回收");
    assert!(!stderr_path.exists(), "淘汰后 stderr 文件应回收");
}
