//! T013.1：diff 展示元信息（接缝 7；清单 1、2）——DSH fs/tool-fs/tests/diff.spec.ts 翻成 Rust。

use std::fs;

use serde_json::{Value, json};
use tools::diff::{FileDiff, compute_hunk_diffs};
use tools::{ApplyPatchTool, EditTool, Tool, ToolCall, ToolContext, ToolResult, WriteTool};

fn diff(path: &str, old: Option<&str>, new: &str) -> FileDiff {
    FileDiff {
        path: path.to_owned(),
        old_text: old.map(str::to_owned),
        new_text: new.to_owned(),
    }
}

async fn run(tool: &dyn Tool, context: &ToolContext, arguments: Value) -> ToolResult {
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

#[test]
fn single_line_change_yields_one_hunk_with_three_context_lines_each_side() {
    let before = "line1\nline2\nline3\nline4\nline5\nline6\nline7\n";
    let after = "line1\nline2\nline3\nchanged\nline5\nline6\nline7\n";
    assert_eq!(
        compute_hunk_diffs("f.txt", before, after),
        vec![diff(
            "f.txt",
            Some("line1\nline2\nline3\nline4\nline5\nline6\nline7"),
            "line1\nline2\nline3\nchanged\nline5\nline6\nline7",
        )]
    );
}

#[test]
fn scattered_replacements_are_one_diff_per_hunk() {
    let before = "line1\nline2\nline3\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nline16\n";
    let after = "line1\nline2\nA\nline4\nline5\nline6\nline7\nline8\nline9\nline10\nline11\nline12\nline13\nline14\nline15\nB\n";
    let diffs = compute_hunk_diffs("f.txt", before, after);
    assert_eq!(diffs.len(), 2, "散的两处改动是两个 hunk：{diffs:?}");
    assert!(diffs[0].new_text.contains("A"));
    assert!(!diffs[0].new_text.contains("B"));
    assert!(diffs[1].new_text.contains("B"));
    assert!(!diffs[1].new_text.contains("A"));
}

#[test]
fn identical_texts_yield_no_hunks() {
    assert!(compute_hunk_diffs("f.txt", "same\n", "same\n").is_empty());
}

#[test]
fn pure_insertion_into_empty_content_reports_old_text_null() {
    assert_eq!(
        compute_hunk_diffs("f.txt", "", "brand new\n"),
        vec![diff("f.txt", None, "brand new")]
    );
}

#[test]
fn pure_deletion_of_the_whole_file_reports_empty_new_text() {
    assert_eq!(
        compute_hunk_diffs("f.txt", "gone\n", ""),
        vec![diff("f.txt", Some("gone"), "")]
    );
}

#[test]
fn no_trailing_newline_marker_is_not_part_of_the_diff() {
    let diffs = compute_hunk_diffs("f.txt", "x", "y");
    assert_eq!(diffs, vec![diff("f.txt", Some("x"), "y")]);
    assert!(!diffs[0].old_text.as_deref().unwrap_or("").contains('\\'));
}

#[tokio::test]
async fn write_new_file_meta_has_create_operation_and_null_old_text() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let tool = WriteTool::new();
    let result = run(
        &tool,
        &context,
        json!({"file_path": "notes/a.txt", "content": "hello\n"}),
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    let meta = result.meta.expect("write 新建要给 meta");
    assert_eq!(meta["operation"], "create");
    assert_eq!(
        meta["path"],
        Value::String(canonical(dir.path(), "notes/a.txt"))
    );
    // 新建也要出 diff：oldText=null、newText 是整份内容（卡片按新增画）。
    assert_eq!(meta["diffs"][0]["oldText"], Value::Null);
    assert_eq!(meta["diffs"][0]["newText"], "hello");
}

#[tokio::test]
async fn write_overwrite_meta_carries_the_hunk_diff() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let tool = WriteTool::new();
    let _ = run(
        &tool,
        &context,
        json!({"file_path": "a.txt", "content": "old\n"}),
    )
    .await;
    let result = run(
        &tool,
        &context,
        json!({"file_path": "a.txt", "content": "new\n"}),
    )
    .await;
    let meta = result.meta.expect("write 覆盖要给 meta");
    assert_eq!(meta["operation"], "update");
    assert_eq!(meta["diffs"][0]["oldText"], "old");
    assert_eq!(meta["diffs"][0]["newText"], "new");
}

#[tokio::test]
async fn edit_meta_carries_the_hunk_diff() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let _ = run(
        &WriteTool::new(),
        &context,
        json!({"file_path": "a.txt", "content": "before\n"}),
    )
    .await;
    let tool = EditTool::new();
    let result = run(
        &tool,
        &context,
        json!({"file_path": "a.txt", "old_string": "before", "new_string": "after"}),
    )
    .await;
    let meta = result.meta.expect("edit 要给 meta");
    assert_eq!(meta["diffs"][0]["oldText"], "before");
    assert_eq!(meta["diffs"][0]["newText"], "after");
}

#[tokio::test]
async fn apply_patch_normal_run_meta_has_diffs_for_add_and_update() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    fs::write(dir.path().join("old.txt"), "line1\nline2\nline3\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "old.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let tool = ApplyPatchTool::new();
    let patch = "*** Begin Patch\n*** Add File: new.txt\n+new content\n*** Update File: old.txt\n@@\n-line2\n+changed\n*** End Patch";
    let result = run(&tool, &context, json!({"input": patch})).await;
    assert!(!result.is_error, "{}", result.content);
    let meta = result.meta.expect("apply_patch 要给 meta");
    assert_eq!(meta["stopped"], false, "全部完成 stopped=false：{meta}");
    let diffs = meta["diffs"].as_array().expect("diffs 数组");
    assert_eq!(diffs.len(), 2, "add + update 各一条：{meta}");
    // add：oldText=null、newText 是内容。
    let add = diffs
        .iter()
        .find(|diff| {
            diff["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("new.txt"))
        })
        .expect("add 的 diff");
    assert_eq!(add["oldText"], Value::Null);
    assert_eq!(add["newText"], "new content");
    // update：老/新文本都带上。
    let update = diffs
        .iter()
        .find(|diff| {
            diff["path"]
                .as_str()
                .is_some_and(|path| path.ends_with("old.txt"))
        })
        .expect("update 的 diff");
    assert_eq!(update["oldText"], "line1\nline2\nline3");
    assert_eq!(update["newText"], "line1\nchanged\nline3");
}

#[tokio::test]
async fn apply_patch_stopped_meta_marks_the_partial_run_without_failed_diff() {
    // 第一个操作完成；第二个操作删除失败（父目录只读）→ stopped=true，失败操作不出 diff。
    let dir = tempfile::tempdir().unwrap();
    let locked = dir.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::write(locked.join("victim.txt"), "bye\n").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o555);
        fs::set_permissions(&locked, permissions).unwrap();
    }
    let context = ToolContext::new(dir.path());
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "locked/victim.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let tool = ApplyPatchTool::new();
    let patch = "*** Begin Patch\n*** Add File: done.txt\n+done\n*** Delete File: locked/victim.txt\n*** End Patch";
    let result = run(&tool, &context, json!({"input": patch})).await;
    let meta = result.meta.expect("停下也要给 meta");
    assert_eq!(
        meta["stopped"], true,
        "删除失败 → stopped=true：{meta}（{}）",
        result.content
    );
    let diffs = meta["diffs"].as_array().expect("diffs 数组");
    assert_eq!(
        diffs.len(),
        1,
        "失败的删除不出 diff，只有完成的 add：{meta}"
    );
    assert_eq!(diffs[0]["oldText"], Value::Null);
    // 恢复目录权限，让 tempdir 收得掉。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&locked).unwrap().permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(&locked, permissions).unwrap();
    }
}

fn canonical(root: &std::path::Path, relative: &str) -> String {
    fs::canonicalize(root.join(relative))
        .unwrap()
        .to_string_lossy()
        .into_owned()
}
