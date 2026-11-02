//! 接缝 2：bash 工具与受管环境变量（测试清单第 3 条）。

use std::fs;
use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use tools::{BashTool, Tool, ToolCall, ToolContext, ToolResult, ToolSessionState, ToolSet};

fn session_context(cwd: &Path, data_dir: &Path) -> ToolContext {
    ToolContext::with_session(cwd, Arc::new(ToolSessionState::new(data_dir, "sess-1")))
}

async fn run(arguments: Value, context: &ToolContext) -> ToolResult {
    let tool = BashTool::new();
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

#[tokio::test]
async fn bash_returns_output_and_exit_code() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());

    let result = run(
        json!({"description": "test command", "command": "printf 'hello\\n'"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello\n");
    assert_eq!(result.error_code, None);

    // 退出码 0 不写 marker（照 DSH renderResult：只看非零）。
    let result = run(
        json!({"description": "test command", "command": "true"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "(no output)");
}

#[tokio::test]
async fn bash_nonzero_exit_is_result_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());

    let result = run(
        json!({"description": "test command", "command": "printf 'bad\\n' >&2; exit 7"}),
        &context,
    )
    .await;
    assert!(
        !result.is_error,
        "非零退出是结果，不是错误：{}",
        result.content
    );
    assert_eq!(result.error_code, None);
    assert_eq!(result.content, "[stderr]\nbad\n[exit code: 7]");
}

#[tokio::test]
async fn bash_long_output_keeps_tail_and_full_path() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path());

    // 5000 行、约 150KB：超过每流 64,000 字节上限。
    let command = "i=0; while [ $i -lt 5000 ]; do printf 'line%04d 0123456789abcdefghij\\n' \"$i\"; i=$((i+1)); done";
    let result = run(
        json!({"description": "test command", "command": command}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("[output truncated; full output: "),
        "{:.200}",
        result.content
    );
    assert!(
        result.content.contains("line4999"),
        "{:.200}",
        result.content
    );
    assert!(!result.content.contains("line0000"), "只保留尾部");

    let start = result
        .content
        .find("[output truncated; full output: ")
        .unwrap()
        + "[output truncated; full output: ".len();
    let rest = &result.content[start..];
    let end = rest.find(']').unwrap();
    let path = &rest[..end];
    assert!(
        path.starts_with(
            &data_dir
                .path()
                .join("spill")
                .join(tools::spill::session_dir_name("sess-1"))
                .to_string_lossy()
                .into_owned()
        ),
        "{path}"
    );
    let full = fs::read_to_string(path).unwrap();
    assert!(full.starts_with("line0000 "), "{full:.60}");
    assert!(
        full.ends_with("line4999 0123456789abcdefghij\n"),
        "{full:.60}"
    );
    assert_eq!(full.lines().count(), 5000);
}

#[tokio::test]
async fn bash_managed_env_vars() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path());

    let result = run(
        json!({"description": "test command", "command": "printf '%s\\n' \"$AIOS_SHELL\" \"$AIOS_SESSION_ID\" \"$AIOS_HOME\""}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    let expected = format!("1\nsess-1\n{}\n", data_dir.path().display());
    assert_eq!(result.content, expected);
}

#[test]
fn bash_tool_is_registered_and_exclusive() {
    use tools::ToolConcurrency;
    let builtin = ToolSet::builtin();
    let bash = builtin.get("bash").expect("builtin() 里有 bash");
    assert_eq!(bash.concurrency(), ToolConcurrency::Exclusive);
    // D38：描述不宣称权限受限/沙箱。
    assert!(!bash.description().to_lowercase().contains("sandbox"));
    assert!(!bash.description().to_lowercase().contains("permission"));
}
const SCRUB_CHILD_ENV: &str = "AIOS_TEST_SCRUB_CHILD";

#[tokio::test]
async fn bash_does_not_leak_credential_env() {
    // 父进程环境里有假 DEEPSEEK_API_KEY / AIOS_LEAK_VALUE 时，子进程不能看到。
    // nextest 每个测试独立进程；这里通过 re-exec 当前测试二进制注入父环境。
    if std::env::var_os(SCRUB_CHILD_ENV).is_none() {
        let exe = std::env::current_exe().unwrap();
        let output = std::process::Command::new(&exe)
            .arg("bash_does_not_leak_credential_env")
            .arg("--exact")
            .arg("--nocapture")
            .env(SCRUB_CHILD_ENV, "1")
            .env("DEEPSEEK_API_KEY", "fake-key-not-real")
            .env("AIOS_LEAK_VALUE", "leak-not-allowed")
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
    let context = session_context(dir.path(), dir.path());
    let result = run(
        json!({"description": "test command", "command": "printf 'key=[%s] aios_leak=[%s] no_color=[%s] home=[%s]\\n' \"$DEEPSEEK_API_KEY\" \"$AIOS_LEAK_VALUE\" \"$NO_COLOR\" \"$HOME\"; printf 'path=[%s]\\n' \"${PATH:+set}\""}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        !result.content.contains("fake-key-not-real"),
        "凭据泄漏到工具输出: {}",
        result.content
    );
    assert!(
        !result.content.contains("leak-not-allowed"),
        "AIOS_* 继承未清: {}",
        result.content
    );
    assert!(
        result
            .content
            .starts_with("key=[] aios_leak=[] no_color=[1] home=["),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("path=[set]"),
        "PATH 应保留: {}",
        result.content
    );
}
#[tokio::test]
async fn bash_stdin_is_closed() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());
    // stdin 是 null：cat 立刻 EOF，不会挂住。
    let result = run(
        json!({"description": "test command", "command": "cat; echo done"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "done\n");
}

#[tokio::test]
async fn bash_spawn_failure_is_tool_error() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());
    let result = run(
        json!({"description": "test command", "command": "true", "workdir": "/definitely/not/a/dir"}),
        &context,
    )
    .await;
    assert!(result.is_error);
    assert!(
        result.content.starts_with("Error: cannot run bash in "),
        "{}",
        result.content
    );
}

#[tokio::test]
async fn bash_workdir_override() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("sub");
    fs::create_dir_all(&sub).unwrap();
    fs::write(sub.join("note.txt"), "hello\n").unwrap();
    let context = session_context(dir.path(), dir.path());
    let result = run(
        json!({"description": "test command", "command": "cat note.txt", "workdir": "sub"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello\n");
}

#[tokio::test]
async fn bash_signal_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());
    let result = run(
        json!({"description": "test command", "command": "kill -TERM $$"}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.contains("[killed by signal: SIGTERM]"),
        "{}",
        result.content
    );
}

fn big_err_command() -> &'static str {
    "i=0; while [ $i -lt 4000 ]; do printf 'err%04d 0123456789abcdefghij\\n' \"$i\" >&2; i=$((i+1)); done"
}

fn big_out_command() -> &'static str {
    "i=0; while [ $i -lt 4000 ]; do printf 'out%04d 0123456789abcdefghij\\n' \"$i\"; i=$((i+1)); done"
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

#[tokio::test]
async fn bash_stderr_over_limit_keeps_tail() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path());
    let result = run(
        json!({"description": "test command", "command": big_err_command()}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert!(
        result.content.starts_with("[stderr]\n"),
        "{:.80}",
        result.content
    );
    assert!(
        result.content.contains("err3999"),
        "{:.200}",
        result.content
    );
    assert!(!result.content.contains("err0000"), "只留尾部");
    let path = truncated_path(&result.content, 0);
    let full = fs::read_to_string(path).unwrap();
    assert!(full.starts_with("err0000 "));
    assert_eq!(full.lines().count(), 4000);
}

#[tokio::test]
async fn bash_both_streams_over_limit() {
    let dir = tempfile::tempdir().unwrap();
    let data_dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), data_dir.path());
    let command = format!("{}; {}", big_out_command(), big_err_command());
    let result = run(
        json!({"description": "test command", "command": command}),
        &context,
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        result
            .content
            .matches("[output truncated; full output: ")
            .count(),
        2,
        "stdout 和 stderr 各一条截断通知: {:.300}",
        result.content
    );
    let stdout_path = truncated_path(&result.content, 0);
    let stderr_path = truncated_path(&result.content, 1);
    assert_ne!(stdout_path, stderr_path);
    assert!(
        fs::read_to_string(stdout_path)
            .unwrap()
            .starts_with("out0000 ")
    );
    assert!(
        fs::read_to_string(stderr_path)
            .unwrap()
            .starts_with("err0000 ")
    );
}

#[test]
fn bash_schema_requires_description() {
    let tool = BashTool::new();
    let parameters = tool.parameters();
    let properties = parameters["properties"].as_object().unwrap();
    let description = properties
        .get("description")
        .expect("DSH tool-bash 的 description 参数是 required");
    assert!(
        description["description"]
            .as_str()
            .unwrap()
            .contains("5-10 words"),
        "description 文案照 DSH：{description}"
    );
    let required = parameters["required"].as_array().unwrap();
    assert!(required.iter().any(|value| value == "description"));
    assert!(required.iter().any(|value| value == "command"));
}

#[tokio::test]
async fn bash_missing_description_is_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());
    let result = run(json!({"command": "true"}), &context).await;
    assert!(
        result.is_error,
        "缺 description 应被拒绝：{}",
        result.content
    );
    assert!(result.content.contains("description"), "{}", result.content);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 1)]
async fn bash_does_not_block_runtime_while_command_runs() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());
    let counter = Arc::new(AtomicUsize::new(0));
    let ticker = {
        let counter = Arc::clone(&counter);
        tokio::spawn(async move {
            loop {
                counter.fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
    };
    // 单 worker runtime：bash 若同步阻塞等待，heartbeat 一次都跑不了。
    let marker = dir.path().join("go");
    let marker_thread = {
        let marker = marker.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            fs::write(marker, "go").unwrap();
        })
    };
    let command = format!(
        "while [ ! -f {} ]; do sleep 0.01; done; printf done",
        marker.display()
    );
    let result = run(
        json!({"description": "wait for marker", "command": command}),
        &context,
    )
    .await;
    ticker.abort();
    marker_thread.join().unwrap();
    assert!(!result.is_error, "{}", result.content);
    assert!(
        counter.load(Ordering::SeqCst) > 0,
        "bash 运行期间 runtime 应能处理其他任务（当前实现同步阻塞 worker）"
    );
}
