//! 写入范围、要求版本和用户接受：task 行、task_update、接受记录、旧行空读。

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::params;

use super::Store;
use crate::{
    Acceptance, ConversationId, ModelChoice, NewReport, NewTask, ReportOrigin, ReportState,
    RequirementVersion, Stage, TaskError, TaskPolicy, VersionSource, Worktree,
};

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

/// 已 canonicalize 的工作目录（生产侧受理任务时也 canonicalize）。
fn task_cwd() -> (tempfile::TempDir, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let cwd = fs::canonicalize(directory.path()).unwrap();
    (directory, cwd)
}

fn draft(cwd: &Path) -> NewTask {
    NewTask {
        title: "改一下判重".to_owned(),
        requirement: "把重复判断改成按 url".to_owned(),
        dod: "同一个 url 只留一条".to_owned(),
        cwd: cwd.to_path_buf(),
        conversation: ConversationId::new("kind-session-1"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn policy(scope: Option<&str>) -> TaskPolicy {
    TaskPolicy {
        write_scope: scope.map(str::to_owned),
        worktree: false,
        bash: false,
    }
}

fn ready_report() -> NewReport {
    NewReport {
        state: ReportState::Ready,
        origin: ReportOrigin::Agent,
        summary: "改完了".to_owned(),
        question: None,
    }
}

#[test]
fn task_accept_freezes_the_write_scope_bash_and_worktree() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let policy = TaskPolicy {
        write_scope: Some("notes".to_owned()),
        worktree: true,
        bash: true,
    };

    let task = store.create_task_with(draft(&cwd), policy, T0).unwrap();
    let scope = cwd.join("notes");
    assert_eq!(task.write_scope.as_deref(), Some(scope.as_path()));
    assert!(task.bash);
    assert!(task.worktree);
    assert_eq!(task.requirement_version(), 1);
    assert_eq!(
        task.versions,
        vec![RequirementVersion {
            write_scope: Some(scope),
            at: T0,
            source: VersionSource::TaskAccept,
        }]
    );
    assert_eq!(store.task(&task.id).unwrap(), Some(task));
}

#[test]
fn a_task_without_a_scope_is_read_only_and_gets_a_first_version() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();

    let task = store.create_task(draft(&cwd), T0).unwrap();
    assert_eq!(task.write_scope, None);
    assert!(!task.bash);
    assert!(!task.worktree);
    assert_eq!(
        task.versions,
        vec![RequirementVersion {
            write_scope: None,
            at: T0,
            source: VersionSource::TaskAccept,
        }]
    );

    let blank = store
        .create_task_with(draft(&cwd), policy(Some("   ")), T0)
        .unwrap();
    assert_eq!(blank.write_scope, None);
    assert_eq!(blank.versions[0].write_scope, None);
}

#[test]
fn a_relative_scope_resolves_against_the_task_cwd() {
    let (_directory, store) = open_store();
    let (_root_directory, root) = task_cwd();
    let cwd = root.join("project");
    fs::create_dir(&cwd).unwrap();

    let task = store
        .create_task_with(draft(&cwd), policy(Some("../notes")), T0)
        .unwrap();
    assert_eq!(
        task.write_scope.as_deref(),
        Some(root.join("notes").as_path())
    );
}

#[test]
fn a_scope_at_the_filesystem_root_or_the_home_directory_is_refused() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();

    let root = store.create_task_with(draft(&cwd), policy(Some("/")), T0);
    assert!(
        matches!(root, Err(TaskError::WriteScopeIsRoot)),
        "根目录要拒: {root:?}"
    );

    let home = std::env::var("HOME").expect("测试环境有 HOME");
    let home = fs::canonicalize(&home).unwrap_or_else(|_| PathBuf::from(home));
    let home_scope = home.to_string_lossy().into_owned();
    let result = store.create_task_with(draft(&cwd), policy(Some(&home_scope)), T0);
    assert!(
        matches!(result, Err(TaskError::WriteScopeIsHome)),
        "用户主目录要拒: {result:?}"
    );
    assert!(store.task_ids().unwrap().is_empty());
}

#[test]
fn task_update_appends_a_requirement_version_and_moves_the_scope() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let task = store
        .create_task_with(draft(&cwd), policy(Some("notes")), T0)
        .unwrap();
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);

    let updated = store.update_write_scope(&task.id, "rank", T0 + 1).unwrap();
    assert_eq!(
        updated.write_scope.as_deref(),
        Some(cwd.join("rank").as_path())
    );
    assert_eq!(updated.requirement_version(), 2);
    assert_eq!(
        updated.versions,
        vec![
            RequirementVersion {
                write_scope: Some(cwd.join("notes")),
                at: T0,
                source: VersionSource::TaskAccept,
            },
            RequirementVersion {
                write_scope: Some(cwd.join("rank")),
                at: T0 + 1,
                source: VersionSource::UserAgreed,
            },
        ]
    );
    assert_eq!(store.task(&task.id).unwrap(), Some(updated));
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);
}

#[test]
fn task_update_refuses_an_empty_scope() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let task = store
        .create_task_with(draft(&cwd), policy(Some("notes")), T0)
        .unwrap();

    let result = store.update_write_scope(&task.id, "   ", T0 + 1);
    assert!(
        matches!(result, Err(TaskError::EmptyWriteScope)),
        "{result:?}"
    );
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.requirement_version(), 1);
    assert_eq!(
        stored.write_scope.as_deref(),
        Some(cwd.join("notes").as_path())
    );
}

#[test]
fn acceptance_binds_the_current_version_and_the_report_without_changing_the_stage() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let task = store
        .create_task_with(draft(&cwd), policy(Some("notes")), T0)
        .unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    let report = store
        .record_report(&task.id, attempt, ready_report(), T0 + 2)
        .unwrap();
    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);

    let acceptance = store.accept(&task.id, T0 + 3).unwrap();
    assert_eq!(
        acceptance,
        Acceptance {
            version: 1,
            report_id: report,
            at: T0 + 3,
        }
    );
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.acceptance, Some(acceptance));
    assert_eq!(stored.requirement_version(), 1);
    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);
}

#[test]
fn accepting_after_a_task_update_binds_the_new_version() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let task = store
        .create_task_with(draft(&cwd), policy(Some("notes")), T0)
        .unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    let report = store
        .record_report(&task.id, attempt, ready_report(), T0 + 2)
        .unwrap();
    store.update_write_scope(&task.id, "rank", T0 + 3).unwrap();

    let acceptance = store.accept(&task.id, T0 + 4).unwrap();
    assert_eq!(acceptance.version, 2);
    assert_eq!(acceptance.report_id, report);
}

#[test]
fn accepting_without_a_report_is_refused() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let task = store
        .create_task_with(draft(&cwd), policy(Some("notes")), T0)
        .unwrap();

    let result = store.accept(&task.id, T0 + 1);
    assert!(
        matches!(result, Err(TaskError::NoReportToAccept)),
        "{result:?}"
    );
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.acceptance, None);
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);
}

#[test]
fn updating_a_worktree_task_to_an_outside_scope_is_refused() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let (_outside_directory, outside) = task_cwd();
    let task = store
        .create_task_with(draft(&cwd), policy(Some("notes")), T0)
        .unwrap();
    let task = store
        .attach_worktree(
            &task.id,
            Worktree {
                root: cwd.join("worktree"),
                repo: cwd.clone(),
                cwd: cwd.join("worktree"),
            },
        )
        .unwrap();
    let before = task.write_scope.clone();

    let result = store.update_write_scope(
        &task.id,
        outside.join("outside.txt").to_str().unwrap(),
        T0 + 1,
    );
    assert!(
        matches!(result, Err(TaskError::WriteScopeOutsideWorktree(_))),
        "{result:?}"
    );
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(after.write_scope, before, "范围没被改掉");
    assert_eq!(after.requirement_version(), 1, "没追加版本");
}

#[test]
fn old_task_rows_without_the_new_fields_read_as_empty() {
    let (_directory, store) = open_store();
    let (_cwd_directory, cwd) = task_cwd();
    let policy = TaskPolicy {
        write_scope: Some("notes".to_owned()),
        worktree: true,
        bash: true,
    };
    let task = store.create_task_with(draft(&cwd), policy, T0).unwrap();

    {
        let conn = store.conn.lock().unwrap();
        let raw: String = conn
            .query_row(
                "SELECT value FROM task WHERE id = ?1",
                params![task.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
        for key in [
            "write_scope",
            "bash",
            "worktree",
            "worktree_state",
            "versions",
            "acceptance",
        ] {
            value.as_object_mut().unwrap().remove(key);
        }
        conn.execute(
            "UPDATE task SET value = ?1 WHERE id = ?2",
            params![value.to_string(), task.id.as_str()],
        )
        .unwrap();
    }

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.write_scope, None);
    assert!(!loaded.bash);
    assert!(!loaded.worktree);
    assert!(loaded.worktree_state.is_none());
    assert!(loaded.versions.is_empty());
    assert_eq!(loaded.acceptance, None);
    assert_eq!(loaded.requirement_version(), 0);
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);
}
