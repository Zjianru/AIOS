//! 任务工作树：第一次 Attempt 起跑前在 cwd 所在的仓库开一份，之后每个 Attempt 沿用。
//!
//! 工作树的事实（根、原仓库、ThinAgent 的 cwd）记在 tasks 的 Task.worktree_state；
//! 这里只做本地 git 操作和路径换算，不保存任务状态。

use std::path::{Path, PathBuf};

use tasks::{Task, TaskId, Worktree};

/// 开工作树失败：不是 git 仓库（卡上写固定说明），或别的（带 git 的错误原文）。
pub(crate) enum WorktreeIssue {
    NotRepository,
    Failed(String),
}

/// 在任务 cwd 所在的仓库开工作树；只在第一次 Attempt 起跑前调用。
///
/// 路径是 <数据目录>/worktrees/<taskId>，分支 aios/<taskId>；cwd 换到工作树里
/// 相同的相对位置。git 已经开出来、落库却没成功时（重试路径）复用已登记的那份，
/// 不再重复 add。
pub(crate) async fn create_for(task: &Task, data_dir: &Path) -> Result<Worktree, WorktreeIssue> {
    let cwd = task.cwd.clone();
    let write_scope = task.write_scope.clone();
    let task_id = task.id.clone();
    let data_dir = data_dir.to_path_buf();
    tokio::task::spawn_blocking(move || create(&cwd, write_scope.as_deref(), &task_id, &data_dir))
        .await
        .map_err(|error| WorktreeIssue::Failed(format!("开工作树的任务没跑完：{error}")))?
}

/// 同步部分：读仓库根、开工作树、算对应子目录。
fn create(
    cwd: &Path,
    write_scope: Option<&Path>,
    task_id: &TaskId,
    data_dir: &Path,
) -> Result<Worktree, WorktreeIssue> {
    let cwd = std::fs::canonicalize(cwd).map_err(|error| {
        WorktreeIssue::Failed(format!("工作目录解析不了：{}: {error}", cwd.display()))
    })?;
    let repo = match git_output(&cwd, &["rev-parse", "--show-toplevel"]) {
        Ok(text) => PathBuf::from(text.trim()),
        Err(message) if message.contains("not a git repository") => {
            return Err(WorktreeIssue::NotRepository);
        }
        Err(message) => {
            return Err(WorktreeIssue::Failed(format!("读不了仓库根：{message}")));
        }
    };
    let repo = std::fs::canonicalize(&repo).map_err(|error| {
        WorktreeIssue::Failed(format!("仓库根解析不了：{}: {error}", repo.display()))
    })?;
    // 写入范围必须落在原仓库里才换得进工作树；换不进去就不开会话，免得不经工作树
    // 写到原目录（spec 停止条件 3）。
    if let Some(scope) = write_scope
        && !scope.starts_with(&repo)
    {
        return Err(WorktreeIssue::Failed(format!(
            "写入范围不在仓库里，工作树换算会落到工作树外面：{}",
            scope.display()
        )));
    }
    let Ok(relative) = cwd.strip_prefix(&repo) else {
        return Err(WorktreeIssue::Failed(format!(
            "工作目录不在仓库里：{}（仓库 {}）",
            cwd.display(),
            repo.display()
        )));
    };
    let root = data_dir.join("worktrees").join(task_id.as_str());
    let branch = format!("aios/{task_id}");
    if root.exists() {
        let root_real = std::fs::canonicalize(&root).map_err(|error| {
            WorktreeIssue::Failed(format!("工作树路径解析不了：{}: {error}", root.display()))
        })?;
        let listed = git_output(&repo, &["worktree", "list", "--porcelain"])
            .map_err(|message| WorktreeIssue::Failed(format!("读不了工作树列表：{message}")))?;
        let registered = listed
            .lines()
            .filter_map(|line| line.strip_prefix("worktree "))
            .any(|path| {
                std::fs::canonicalize(path)
                    .ok()
                    .as_deref()
                    .is_some_and(|path| path == root_real)
            });
        if !registered {
            return Err(WorktreeIssue::Failed(format!(
                "工作树路径已存在，但不是这个仓库的工作树：{}",
                root.display()
            )));
        }
    } else {
        let root_text = root.to_string_lossy().into_owned();
        let branch_ref = format!("refs/heads/{branch}");
        let branch_exists =
            git_output(&repo, &["show-ref", "--verify", "--quiet", &branch_ref]).is_ok();
        let args: Vec<&str> = if branch_exists {
            vec!["worktree", "add", &root_text, &branch]
        } else {
            vec!["worktree", "add", &root_text, "-b", &branch]
        };
        git_output(&repo, &args).map_err(|message| {
            WorktreeIssue::Failed(format!("开不了工作树（{}）：{message}", root.display()))
        })?;
    }
    let root = std::fs::canonicalize(&root).map_err(|error| {
        WorktreeIssue::Failed(format!("工作树路径解析不了：{}: {error}", root.display()))
    })?;
    let cwd = root.join(relative);
    if !cwd.is_dir() {
        return Err(WorktreeIssue::Failed(format!(
            "工作树里没有对应的目录：{}",
            cwd.display()
        )));
    }
    let cwd = std::fs::canonicalize(&cwd).map_err(|error| {
        WorktreeIssue::Failed(format!(
            "工作树里的 cwd 解析不了：{}: {error}",
            cwd.display()
        ))
    })?;
    Ok(Worktree { root, repo, cwd })
}

/// 在目录里跑一条 git；成功返回 stdout，失败返回去掉首尾空白的 stderr。
///
/// LC_ALL=C 让「不是仓库」的报错保持可识别。git 在本地、命令短，放在阻塞池里跑。
fn git_output(dir: &Path, args: &[&str]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("LC_ALL", "C")
        .output()
        .map_err(|error| format!("跑不了 git：{error}"))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_owned())
    }
}
