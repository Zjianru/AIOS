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

    let result = run(json!({"command": "printf 'hello\\n'"}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "hello\n");
    assert_eq!(result.error_code, None);

    // 退出码 0 不写 marker（照 DSH renderResult：只看非零）。
    let result = run(json!({"command": "true"}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "(no output)");
}

#[tokio::test]
async fn bash_nonzero_exit_is_result_not_error() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path(), dir.path());

    let result = run(json!({"command": "printf 'bad\\n' >&2; exit 7"}), &context).await;
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
    let result = run(json!({"command": command}), &context).await;
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
                .join("session-sess-1")
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
        json!({"command": "printf '%s\\n' \"$AIOS_SHELL\" \"$AIOS_SESSION_ID\" \"$AIOS_HOME\""}),
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
