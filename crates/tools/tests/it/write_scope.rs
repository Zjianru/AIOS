//! 接缝 1：写类工具按写入范围核目标（D85；write / edit / apply_patch 都接）。
//!
//! 范围是 canonicalize 后的绝对路径；目标先 resolve 再比，所以 `..`、符号链接、
//! 不存在的新文件都按工具真正要写的那个路径判断。

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;
use tools::{ApplyPatchTool, EditTool, ReadTool, ToolContext, ToolResult, WriteTool};

fn canonical(path: &Path) -> PathBuf {
    fs::canonicalize(path).unwrap()
}

/// cwd 在临时目录；写入范围取 canonicalize 后的路径（生产侧由服务解析）。
fn scoped_context(cwd: &Path, scope: &Path) -> ToolContext {
    crate::common::context(cwd).with_write_scope(Some(canonical(scope)))
}

fn out_of_scope_message(scope: &Path) -> String {
    format!(
        "Error: 这个路径不在写入范围里（写入范围：{}）。要改范围外的，调 task_handback。",
        scope.display()
    )
}

async fn write(context: &ToolContext, path: &str, content: &str) -> ToolResult {
    crate::common::run(
        &WriteTool::new(),
        json!({"file_path": path, "content": content}),
        context,
    )
    .await
}

async fn read(context: &ToolContext, path: &str) -> ToolResult {
    crate::common::run(&ReadTool::new(), json!({"file_path": path}), context).await
}

#[tokio::test]
async fn a_file_scope_allows_only_that_file() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let file = cwd.join("notes.txt");
    fs::write(&file, "old\n").unwrap();
    let context = scoped_context(cwd, &file);
    let scope = canonical(&file);

    let read = read(&context, "notes.txt").await;
    assert!(!read.is_error, "{}", read.content);
    let updated = write(&context, "notes.txt", "new\n").await;
    assert!(!updated.is_error, "{}", updated.content);
    assert_eq!(fs::read_to_string(&file).unwrap(), "new\n");

    let outside = write(&context, "other.txt", "x\n").await;
    assert!(outside.is_error, "{}", outside.content);
    assert_eq!(outside.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(outside.content, out_of_scope_message(&scope));
    assert!(!cwd.join("other.txt").exists());
}

#[tokio::test]
async fn a_directory_scope_allows_new_files_under_it() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let work = cwd.join("work");
    fs::create_dir(&work).unwrap();
    let context = scoped_context(cwd, &work);
    let scope = canonical(&work);

    let created = write(&context, "work/notes/first.txt", "hi\n").await;
    assert!(!created.is_error, "{}", created.content);
    assert_eq!(
        fs::read_to_string(work.join("notes/first.txt")).unwrap(),
        "hi\n"
    );

    let outside = write(&context, "outside.txt", "x\n").await;
    assert!(outside.is_error, "{}", outside.content);
    assert_eq!(outside.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(outside.content, out_of_scope_message(&scope));
    assert!(!cwd.join("outside.txt").exists());
}

#[tokio::test]
async fn a_new_file_can_be_written_when_it_is_the_scope() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let scope = canonical(cwd).join("new.txt");
    let context = crate::common::context(cwd).with_write_scope(Some(scope.clone()));

    let created = write(&context, "new.txt", "hi\n").await;
    assert!(!created.is_error, "{}", created.content);
    assert_eq!(fs::read_to_string(cwd.join("new.txt")).unwrap(), "hi\n");

    let outside = write(&context, "other.txt", "x\n").await;
    assert!(outside.is_error, "{}", outside.content);
    assert_eq!(outside.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(outside.content, out_of_scope_message(&scope));
    assert!(!cwd.join("other.txt").exists());
}

#[tokio::test]
async fn parent_traversal_out_of_the_scope_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let work = cwd.join("work");
    fs::create_dir(&work).unwrap();
    let context = scoped_context(cwd, &work);

    let escaped = write(&context, "work/../escape.txt", "x\n").await;
    assert!(escaped.is_error, "{}", escaped.content);
    assert_eq!(escaped.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(escaped.content, out_of_scope_message(&canonical(&work)));
    assert!(!cwd.join("escape.txt").exists());
}

#[tokio::test]
async fn a_symlink_pointing_outside_the_scope_is_rejected() {
    use std::os::unix::fs::symlink;

    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let work = cwd.join("work");
    fs::create_dir(&work).unwrap();
    let outside = tempfile::tempdir().unwrap();
    symlink(outside.path(), work.join("link")).unwrap();
    let context = scoped_context(cwd, &work);

    let escaped = write(&context, "work/link/written.txt", "x\n").await;
    assert!(escaped.is_error, "{}", escaped.content);
    assert_eq!(escaped.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(escaped.content, out_of_scope_message(&canonical(&work)));
    assert!(!outside.path().join("written.txt").exists());
}

#[tokio::test]
async fn git_metadata_under_the_scope_is_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let repository = cwd.join("repo");
    fs::create_dir_all(repository.join(".git")).unwrap();
    fs::write(repository.join(".git/config"), "old\n").unwrap();
    let context = scoped_context(cwd, &repository);
    let scope = canonical(&repository);

    let result = write(&context, "repo/.git/config", "new\n").await;
    assert!(result.is_error, "{}", result.content);
    assert_eq!(result.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(result.content, out_of_scope_message(&scope));
    assert_eq!(
        fs::read_to_string(repository.join(".git/config")).unwrap(),
        "old\n"
    );

    // 子模块/worktree 的 .git 是指针文件，也一样不放行。
    let pointer_cwd = tempfile::tempdir().unwrap();
    let pointer = pointer_cwd.path().join("repo");
    fs::create_dir(&pointer).unwrap();
    fs::write(pointer.join(".git"), "gitdir: ../elsewhere\n").unwrap();
    let pointer_context = scoped_context(pointer_cwd.path(), &pointer);
    let rejected = write(&pointer_context, "repo/.git", "gitdir: /tmp/x\n").await;
    assert!(rejected.is_error, "{}", rejected.content);
    assert_eq!(rejected.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(
        fs::read_to_string(pointer.join(".git")).unwrap(),
        "gitdir: ../elsewhere\n"
    );
}

#[tokio::test]
async fn a_sibling_with_the_scope_prefix_is_not_inside_the_scope() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let scope_dir = cwd.join("a").join("b");
    fs::create_dir_all(&scope_dir).unwrap();
    let sibling = cwd.join("a").join("bc");
    fs::create_dir_all(&sibling).unwrap();
    let context = scoped_context(cwd, &scope_dir);

    let rejected = write(&context, "a/bc/x.txt", "x\n").await;
    assert!(rejected.is_error, "{}", rejected.content);
    assert_eq!(rejected.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(
        rejected.content,
        out_of_scope_message(&canonical(&scope_dir))
    );
    assert!(!sibling.join("x.txt").exists());

    let allowed = write(&context, "a/b/x.txt", "ok\n").await;
    assert!(!allowed.is_error, "{}", allowed.content);
}

#[tokio::test]
async fn edit_honours_the_scope() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let work = cwd.join("work");
    fs::create_dir(&work).unwrap();
    let inside = work.join("a.txt");
    fs::write(&inside, "old\n").unwrap();
    let outside = cwd.join("victim.txt");
    fs::write(&outside, "old\n").unwrap();
    let context = scoped_context(cwd, &work);

    let read = read(&context, "work/a.txt").await;
    assert!(!read.is_error, "{}", read.content);
    let edited = crate::common::run(
        &EditTool::new(),
        json!({"file_path": "work/a.txt", "old_string": "old", "new_string": "new"}),
        &context,
    )
    .await;
    assert!(!edited.is_error, "{}", edited.content);
    assert_eq!(fs::read_to_string(&inside).unwrap(), "new\n");

    let rejected = crate::common::run(
        &EditTool::new(),
        json!({"file_path": "victim.txt", "old_string": "old", "new_string": "new"}),
        &context,
    )
    .await;
    assert!(rejected.is_error, "{}", rejected.content);
    assert_eq!(rejected.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    assert_eq!(rejected.content, out_of_scope_message(&canonical(&work)));
    assert_eq!(fs::read_to_string(&outside).unwrap(), "old\n");
}

#[tokio::test]
async fn apply_patch_checks_every_path_before_any_change() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let work = cwd.join("work");
    fs::create_dir(&work).unwrap();
    let inside = work.join("a.txt");
    fs::write(&inside, "one\n").unwrap();
    let context = scoped_context(cwd, &work);
    let scope = canonical(&work);

    let read = read(&context, "work/a.txt").await;
    assert!(!read.is_error, "{}", read.content);

    let patch = "*** Begin Patch\n*** Update File: work/a.txt\n@@\n one\n+two\n*** Add File: outside.txt\n+hi\n*** End Patch";
    let result =
        crate::common::run(&ApplyPatchTool::new(), json!({ "input": patch }), &context).await;
    assert!(result.is_error, "{}", result.content);
    assert_eq!(result.error_code.as_deref(), Some("WRITE_OUT_OF_SCOPE"));
    let outside = canonical(cwd).join("outside.txt");
    let scope_message = format!(
        "这个路径不在写入范围里（写入范围：{}）。要改范围外的，调 task_handback。",
        scope.display()
    );
    assert_eq!(
        result.content,
        format!(
            "Error: Patch rejected before any change (1 problems):\n- operation 2 (add {}, patch line 6): {scope_message}",
            outside.display()
        )
    );
    assert_eq!(fs::read_to_string(&inside).unwrap(), "one\n");
    assert!(!cwd.join("outside.txt").exists());
}

#[tokio::test]
async fn a_context_without_a_scope_writes_where_it_used_to() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let context = crate::common::context(cwd);

    let created = write(&context, "anywhere/new.txt", "hi\n").await;
    assert!(!created.is_error, "{}", created.content);
    assert_eq!(
        fs::read_to_string(cwd.join("anywhere/new.txt")).unwrap(),
        "hi\n"
    );
}
