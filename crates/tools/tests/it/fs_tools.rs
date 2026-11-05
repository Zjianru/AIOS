//! 接缝 2：写类工具 write / edit / apply_patch 与编辑前先读（测试清单第 1、2 条 + D5 加项）。

use std::fs;
use std::path::Path;

use serde_json::{Value, json};
use tools::{ApplyPatchTool, EditTool, Tool, ToolCall, ToolContext, ToolResult, WriteTool};

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

fn canonical(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap()
        .to_string_lossy()
        .into_owned()
}

#[tokio::test]
async fn write_creates_file() {
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
    let real = canonical(&dir.path().join("notes/a.txt"));
    assert_eq!(
        result.content,
        format!("<path>{real}</path>\n<type>file</type>\n<content>\nCreated file\n</content>")
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("notes/a.txt")).unwrap(),
        "hello\n"
    );
    assert_eq!(result.error_code, None);
}
use std::sync::atomic::AtomicBool;

use tools::{ToolSet, execute_batch};

#[tokio::test]
async fn batch_write_before_read_is_rejected() {
    // D5（票面加项）：批内按模型顺序执行；写在前时观察表还没有记录，写被拒。
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, "old\n").unwrap();
    let context = ToolContext::new(dir.path());
    let tools = ToolSet::builtin();
    let calls = vec![
        ToolCall {
            id: "call_0".to_owned(),
            name: "write".to_owned(),
            arguments: json!({"file_path": "a.txt", "content": "new\n"}).to_string(),
        },
        ToolCall {
            id: "call_1".to_owned(),
            name: "read".to_owned(),
            arguments: json!({"file_path": "a.txt"}).to_string(),
        },
    ];
    let outcome = execute_batch(&tools, &calls, 10, &context, &AtomicBool::new(false)).await;
    assert_eq!(outcome.results.len(), 2);
    let write = &outcome.results[0];
    assert!(write.is_error, "{}", write.content);
    assert_eq!(write.error_code.as_deref(), Some("FS_NOT_OBSERVED"));
    assert_eq!(
        write.content,
        format!(
            "Error: cannot modify \"{}\": file has not been read — read the file, then retry",
            canonical(&file)
        )
    );
    assert!(
        !outcome.results[1].is_error,
        "{}",
        outcome.results[1].content
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "old\n");
}
#[tokio::test]
async fn read_then_write_overwrites_file() {
    // 测试清单第 1、2 条：读过之后才允许覆盖；写成功记 Updated file。
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "old\n").unwrap();

    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let result = run(
        &WriteTool::new(),
        &context,
        json!({"file_path": "a.txt", "content": "new\n"}),
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    let real = canonical(&file);
    assert_eq!(
        result.content,
        format!("<path>{real}</path>\n<type>file</type>\n<content>\nUpdated file\n</content>")
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "new\n");
}
#[tokio::test]
async fn write_unread_existing_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "old\n").unwrap();

    let result = run(
        &WriteTool::new(),
        &context,
        json!({"file_path": "a.txt", "content": "new\n"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_OBSERVED"));
    assert_eq!(
        result.content,
        format!(
            "Error: cannot modify \"{}\": file has not been read — read the file, then retry",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "old\n");
}

#[tokio::test]
async fn write_after_external_change_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "old\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    // 读完之后文件被外部改了：版本 token 变，写被拒。
    fs::write(&file, "external\n").unwrap();

    let result = run(
        &WriteTool::new(),
        &context,
        json!({"file_path": "a.txt", "content": "ours\n"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_STALE_VERSION"));
    assert_eq!(
        result.content,
        format!(
            "Error: cannot write \"{}\": file changed since it was read — re-read the file, then retry",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "external\n");
}
#[tokio::test]
async fn edit_replaces_unique_match() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "alpha\nbeta\ngamma\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "beta", "new_string": "BETA"}),
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        result.content,
        format!(
            "The file {} has been updated successfully.",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nBETA\ngamma\n");
}
#[tokio::test]
async fn edit_reports_missing_match() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "alpha\nbeta\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "nope", "new_string": "x"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_EDIT_NOT_FOUND"));
    assert_eq!(
        result.content,
        format!(
            "Error: old_string was not found in \"{}\"",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\nbeta\n");
}

#[tokio::test]
async fn edit_reports_ambiguous_match_then_replace_all() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "x\nx\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "x", "new_string": "y"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_AMBIGUOUS_EDIT"));
    assert_eq!(
        result.content,
        format!(
            "Error: old_string matched 2 times in \"{}\"; provide a more specific old_string or set replace_all to true",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "x\nx\n");

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "x", "new_string": "y", "replace_all": true}),
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        result.content,
        format!(
            "The file {} has been updated. All occurrences were successfully replaced.",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "y\ny\n");
}

#[tokio::test]
async fn edit_unread_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "alpha\n").unwrap();

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "alpha", "new_string": "beta"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_OBSERVED"));
    assert_eq!(
        result.content,
        format!(
            "Error: cannot modify \"{}\": file has not been read — read the file, then retry",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha\n");
}

#[tokio::test]
async fn edit_after_external_change_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "alpha\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    fs::write(&file, "externally changed\n").unwrap();

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "alpha", "new_string": "beta"}),
    )
    .await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_STALE_VERSION"));
    assert_eq!(
        result.content,
        format!(
            "Error: cannot edit \"{}\": file changed since it was read — re-read the file, then retry",
            canonical(&file)
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "externally changed\n");
}

#[tokio::test]
async fn edit_preserves_crlf_line_endings() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, b"one\r\ntwo\r\nthree\r\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let result = run(
        &EditTool::new(),
        &context,
        json!({"file_path": "a.txt", "old_string": "two", "new_string": "TWO"}),
    )
    .await;
    assert!(!result.is_error, "{}", result.content);
    // D4：匹配按 LF 归一，写回恢复文件原来的 CRLF 行尾。
    assert_eq!(fs::read(&file).unwrap(), b"one\r\nTWO\r\nthree\r\n");
}
async fn patch_run(arguments: Value, context: &ToolContext) -> ToolResult {
    run(&ApplyPatchTool::new(), context, arguments).await
}

#[tokio::test]
async fn patch_rejects_syntax_error() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());

    let result = patch_run(json!({"input": "not a patch"}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("PATCH_SYNTAX"));
    assert_eq!(
        result.content,
        "Error: Invalid patch at line 1: Expected '*** Begin Patch'"
    );
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[tokio::test]
async fn patch_applies_add_and_update() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\ntwo\nthree\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let patch = "*** Begin Patch\n*** Add File: b.txt\n+hello\n+world\n*** Update File: a.txt\n@@\n one\n-two\n+TWO\n three\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    let a = canonical(&file);
    let b = canonical(&dir.path().join("b.txt"));
    assert_eq!(
        result.content,
        format!("Success. Updated the following files:\nA {b}\nM {a}")
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "one\nTWO\nthree\n");
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "hello\nworld\n"
    );
}

#[tokio::test]
async fn patch_rejects_hunk_mismatch_before_any_change() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\ntwo\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let a = canonical(&file);

    let patch = "*** Begin Patch\n*** Add File: b.txt\n+hello\n*** Update File: a.txt\n@@\n one\n-missing\n+X\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_PATCH_MISMATCH"));
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 2 (update {a}, patch line 4): cannot patch \"{a}\": hunk at patch line 5 (oldLines) does not match — one"
        )
    );
    // 预检拒绝时整包不落盘：Add 也不能生效。
    assert!(!dir.path().join("b.txt").exists());
    assert_eq!(fs::read_to_string(&file).unwrap(), "one\ntwo\n");
}

#[tokio::test]
async fn patch_delete_requires_read() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let a = canonical(&file);

    let patch = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_NOT_OBSERVED"));
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 1 (delete {a}, patch line 2): cannot modify \"{a}\": file has not been read — read the file, then retry"
        )
    );
    assert!(file.exists());
}

#[tokio::test]
async fn patch_empty_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let result = patch_run(json!({"input": "*** Begin Patch\n*** End Patch"}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(result.content, "Success. No operations in patch.");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn write_tools_are_exclusive() {
    use tools::ToolConcurrency;
    assert_eq!(WriteTool::new().concurrency(), ToolConcurrency::Exclusive);
    assert_eq!(EditTool::new().concurrency(), ToolConcurrency::Exclusive);
    assert_eq!(
        ApplyPatchTool::new().concurrency(),
        ToolConcurrency::Exclusive
    );
    assert_eq!(
        tools::ReadTool::new().concurrency(),
        ToolConcurrency::Parallel
    );
}

#[tokio::test]
async fn patch_delete_symlink_source_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let real = dir.path().join("a.txt");
    fs::write(&real, "one\n").unwrap();
    let link = dir.path().join("link.txt");
    symlink(&real, &link).unwrap();
    let display = canonical(&real);

    let patch = "*** Begin Patch\n*** Delete File: link.txt\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("PATCH_SYMLINK_SOURCE"));
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 1 (delete {display}, patch line 2): cannot delete \"{display}\": path is a symbolic link — operate on its target path instead"
        )
    );
    assert!(real.exists());
    assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
}

#[tokio::test]
async fn patch_move_to_same_path_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let a = canonical(&file);

    let patch = "*** Begin Patch\n*** Update File: a.txt\n*** Move to: a.txt\n@@\n-one\n+two\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_SAME_TARGET"));
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 1 (move {a}, patch line 2): cannot move \"{a}\" to itself"
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "one\n");
}

#[tokio::test]
async fn patch_move_to_symlink_alias_is_rejected() {
    use std::os::unix::fs::symlink;
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let link = dir.path().join("link.txt");
    symlink(&file, &link).unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let a = canonical(&file);

    let patch = "*** Begin Patch\n*** Update File: a.txt\n*** Move to: link.txt\n@@\n-one\n+two\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_SAME_TARGET"));
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 1 (move {a}, patch line 2): cannot move \"{a}\" to itself"
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "one\n");
}

#[tokio::test]
async fn patch_move_partial_failure_reports_destination_written() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let src_dir = dir.path().join("src");
    let dst_dir = dir.path().join("dst");
    fs::create_dir_all(&src_dir).unwrap();
    fs::create_dir_all(&dst_dir).unwrap();
    let src = src_dir.join("a.txt");
    fs::write(&src, "one\n").unwrap();
    let context = ToolContext::new(dir.path());
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "src/a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let src_display = canonical(&src);
    let dst_display = canonical(&dst_dir).trim_end_matches('/').to_owned() + "/a.txt";

    // 源目录去掉写权限：目的地写入成功、删源失败（确定性部分失败）。
    fs::set_permissions(&src_dir, fs::Permissions::from_mode(0o555)).unwrap();
    let patch = "*** Begin Patch\n*** Update File: src/a.txt\n*** Move to: dst/a.txt\n@@\n-one\n+two\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    fs::set_permissions(&src_dir, fs::Permissions::from_mode(0o755)).unwrap();

    assert!(!result.is_error, "{}", result.content);
    assert!(
        result
            .content
            .starts_with("Patch stopped after 0 of 1 operations."),
        "{}",
        result.content
    );
    assert!(result.content.contains("Failed:"), "{}", result.content);
    // DSH apply-patch-output.ts:122-123：部分 move 单独一行报「目的地已写、源还在」。
    assert!(
        result.content.lines().any(|line| line
            == format!("  destination {dst_display} was written; source {src_display} remains")),
        "{}",
        result.content
    );
    assert!(result.content.contains(&dst_display), "{}", result.content);
    assert!(result.content.contains(&src_display), "{}", result.content);
    assert_eq!(fs::read_to_string(dst_dir.join("a.txt")).unwrap(), "two\n");
    assert_eq!(fs::read_to_string(&src).unwrap(), "one\n");
}

#[tokio::test]
async fn patch_deletes_read_file() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let a = canonical(&file);

    let patch = "*** Begin Patch\n*** Delete File: a.txt\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    assert_eq!(
        result.content,
        format!("Success. Updated the following files:\nD {a}")
    );
    assert!(!file.exists());
}

#[tokio::test]
async fn patch_moves_file_with_update() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);

    let patch = "*** Begin Patch\n*** Update File: a.txt\n*** Move to: b.txt\n@@\n-one\n+two\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(!result.is_error, "{}", result.content);
    let b = canonical(&dir.path().join("b.txt"));
    assert_eq!(
        result.content,
        format!("Success. Updated the following files:\nM {b}")
    );
    assert!(!file.exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "two\n"
    );
}

#[tokio::test]
async fn patch_after_external_change_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    let file = dir.path().join("a.txt");
    fs::write(&file, "one\n").unwrap();
    let read = run(
        &tools::ReadTool::new(),
        &context,
        json!({"file_path": "a.txt"}),
    )
    .await;
    assert!(!read.is_error, "{}", read.content);
    let a = canonical(&file);
    fs::write(&file, "external\n").unwrap();

    let patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+two\n*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("FS_STALE_VERSION"));
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 1 (update {a}, patch line 2): cannot edit \"{a}\": file changed since it was read — re-read the file, then retry"
        )
    );
    assert_eq!(fs::read_to_string(&file).unwrap(), "external\n");
}

struct BlockingTool {
    started: tokio::sync::mpsc::UnboundedSender<()>,
    release: std::sync::Arc<tokio::sync::Semaphore>,
    parameters: Value,
}

impl BlockingTool {
    fn new(
        started: tokio::sync::mpsc::UnboundedSender<()>,
        release: std::sync::Arc<tokio::sync::Semaphore>,
    ) -> Self {
        Self {
            started,
            release,
            parameters: json!({"type": "object", "properties": {}}),
        }
    }
}

impl Tool for BlockingTool {
    fn name(&self) -> &str {
        "block"
    }

    fn description(&self) -> &str {
        "test-only blocking tool"
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn concurrency(&self) -> tools::ToolConcurrency {
        tools::ToolConcurrency::Parallel
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> tools::ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let _ = self.started.send(());
            let permit = self
                .release
                .clone()
                .acquire_owned()
                .await
                .expect("测试信号量被关闭");
            drop(permit);
            ToolResult::success(call.id, "ok")
        })
    }
}

#[tokio::test]
async fn write_tool_forms_real_batch_barrier() {
    // 真屏障：并行工具还没结束时，独占的 write 不能执行。
    let dir = tempfile::tempdir().unwrap();
    let (started_tx, mut started) = tokio::sync::mpsc::unbounded_channel();
    let release = std::sync::Arc::new(tokio::sync::Semaphore::new(0));
    let tools = ToolSet::new(vec![
        Box::new(BlockingTool::new(started_tx, release.clone())),
        Box::new(WriteTool::new()),
    ]);
    let context = ToolContext::new(dir.path());
    let calls = vec![
        ToolCall {
            id: "call_0".to_owned(),
            name: "block".to_owned(),
            arguments: "{}".to_owned(),
        },
        ToolCall {
            id: "call_1".to_owned(),
            name: "write".to_owned(),
            arguments: json!({"file_path": "a.txt", "content": "hello\n"}).to_string(),
        },
    ];
    let handle = tokio::spawn(async move {
        execute_batch(&tools, &calls, 10, &context, &AtomicBool::new(false)).await
    });
    started.recv().await.expect("block 工具没开始");
    assert!(
        !dir.path().join("a.txt").exists(),
        "write 在并行工具结束前就跑了：没进独占屏障"
    );
    release.add_permits(1);
    let outcome = handle.await.unwrap();
    assert!(
        !outcome.results[0].is_error,
        "{}",
        outcome.results[0].content
    );
    assert!(
        !outcome.results[1].is_error,
        "{}",
        outcome.results[1].content
    );
    assert_eq!(
        fs::read_to_string(dir.path().join("a.txt")).unwrap(),
        "hello\n"
    );
}

/// T006.6：解析失败也汇总进同一条 Patch rejected（DSH patch/plan.ts:22-68 收集 refusals）。
#[tokio::test]
async fn patch_rejected_aggregates_resolution_and_preflight_problems() {
    let dir = tempfile::tempdir().unwrap();
    let context = ToolContext::new(dir.path());
    // op1 解析失败（父路径是普通文件，ENOTDIR）；op2 预检失败（存在但没读过）。
    fs::write(dir.path().join("blocker"), "x").unwrap();
    fs::write(
        dir.path().join("b.txt"),
        "one
",
    )
    .unwrap();
    let patch = "*** Begin Patch
*** Add File: blocker/nested.txt
+hello
*** Update File: b.txt
@@
-one
+two
*** End Patch";
    let result = patch_run(json!({"input": patch}), &context).await;
    assert!(result.is_error, "{}", result.content);
    assert!(
        result
            .content
            .contains("Patch rejected before any change (2 problems)"),
        "解析失败与预检失败要合成一条：{}",
        result.content
    );
    assert!(
        result.content.contains("- operation 1 (add "),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("- operation 2 (update "),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("cannot resolve"),
        "{}",
        result.content
    );
    assert!(
        result.content.contains("file has not been read"),
        "{}",
        result.content
    );
    // 整包拒绝：不落任何改动。
    assert_eq!(
        fs::read_to_string(dir.path().join("b.txt")).unwrap(),
        "one
"
    );
    assert!(!dir.path().join("blocker").is_dir());
}
