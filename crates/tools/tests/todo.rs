//! 接缝 2：todo_write 工具（测试清单第 7 条）。

use std::path::Path;
use std::sync::Arc;

use serde_json::{Value, json};
use session::{TodoItem, TodoStatus};
use tools::{TodoWriteTool, Tool, ToolCall, ToolContext, ToolResult, ToolSessionState};

fn session_context(dir: &Path) -> ToolContext {
    ToolContext::with_session(dir, Arc::new(ToolSessionState::new(dir, "sess-1")))
}

async fn call(tool: &dyn Tool, arguments: Value, context: &ToolContext) -> ToolResult {
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

async fn write_todos(todos: Value, context: &ToolContext) -> ToolResult {
    call(&TodoWriteTool::new(), json!({ "todos": todos }), context).await
}

fn item(content: &str, status: TodoStatus) -> TodoItem {
    TodoItem {
        content: content.to_owned(),
        status,
    }
}

#[tokio::test]
async fn todo_write_replaces_whole_list_and_returns_counts() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path());
    let first = write_todos(
        json!([
            { "content": "plan", "status": "in_progress" },
            { "content": "  write tests  ", "status": "pending" },
            { "content": "ship", "status": "completed" }
        ]),
        &context,
    )
    .await;
    assert!(!first.is_error, "{}", first.content);
    assert_eq!(
        first.content,
        "Updated todo list: 1 pending, 1 in progress, 1 completed."
    );
    assert_eq!(
        context.session().todos(),
        Some(vec![
            item("plan", TodoStatus::InProgress),
            item("write tests", TodoStatus::Pending),
            item("ship", TodoStatus::Completed),
        ]),
        "内容 trim 后保存"
    );

    // 第二次调用整体替换，不是增量。
    let second = write_todos(
        json!([{ "content": "only one", "status": "pending" }]),
        &context,
    )
    .await;
    assert!(!second.is_error, "{}", second.content);
    assert_eq!(
        second.content,
        "Updated todo list: 1 pending, 0 in progress, 0 completed."
    );
    assert_eq!(
        context.session().todos(),
        Some(vec![item("only one", TodoStatus::Pending)]),
        "整表替换"
    );
}

#[tokio::test]
async fn empty_list_clears_the_todo_list() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path());
    let wrote = write_todos(json!([{ "content": "a", "status": "pending" }]), &context).await;
    assert!(!wrote.is_error, "{}", wrote.content);

    let cleared = write_todos(json!([]), &context).await;
    assert!(!cleared.is_error, "{}", cleared.content);
    assert_eq!(
        cleared.content,
        "Updated todo list: 0 pending, 0 in progress, 0 completed."
    );
    assert_eq!(context.session().todos(), Some(Vec::new()), "空列表清空");
}

#[tokio::test]
async fn invalid_todo_entries_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path());
    let cases: Vec<(Value, &str)> = vec![
        (
            json!([{ "content": "   ", "status": "pending" }]),
            "non-empty",
        ),
        (
            json!([
                { "content": "dup", "status": "pending" },
                { "content": "dup", "status": "completed" }
            ]),
            "duplicate",
        ),
        (
            json!([{ "content": "x", "status": "doing" }]),
            "status must be",
        ),
        (
            json!([{ "content": "x", "status": "pending", "children": [] }]),
            "unknown property",
        ),
        (json!("nope"), "expected an array"),
    ];
    for (todos, fragment) in cases {
        let result = write_todos(todos, &context).await;
        assert!(result.is_error, "应拒绝：{}", result.content);
        assert!(
            result.content.contains(fragment),
            "错文案缺 {fragment}：{}",
            result.content
        );
    }
    assert!(context.session().todos().is_none(), "失败调用不改列表状态");
}

// ---- T006.12 红：allowParallelInProgress=false 的单活跃策略（skeleton 上应失败）----

#[tokio::test]
async fn single_active_policy_rejects_two_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let context = session_context(dir.path());
    let tool = TodoWriteTool::new_with_parallel(false);
    let rejected = call(
        &tool,
        json!({"todos": [
            {"content": "a", "status": "in_progress"},
            {"content": "b", "status": "in_progress"}
        ]}),
        &context,
    )
    .await;
    assert!(rejected.is_error, "多 in_progress 应被拒");
    assert_eq!(
        rejected.content,
        "Error: invalid todos: at most one task may be in_progress (got 2)"
    );
    let accepted = call(
        &tool,
        json!({"todos": [
            {"content": "a", "status": "in_progress"},
            {"content": "b", "status": "pending"}
        ]}),
        &context,
    )
    .await;
    assert!(!accepted.is_error, "{}", accepted.content);
    assert_eq!(
        context.session().todos(),
        Some(vec![
            item("a", TodoStatus::InProgress),
            item("b", TodoStatus::Pending)
        ])
    );
}

#[test]
fn description_reflects_parallel_policy() {
    let parallel = TodoWriteTool::new().description().to_owned();
    let single = TodoWriteTool::new_with_parallel(false).description().to_owned();
    assert!(
        parallel.contains("several only when work runs in parallel"),
        "{parallel}"
    );
    assert!(single.contains("keep exactly one todo"), "{single}");
    assert!(
        !single.contains("several only when work runs in parallel"),
        "{single}"
    );
}
