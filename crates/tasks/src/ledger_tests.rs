//! T056.2：schema v2、约束跟着要求版本、账本叙述条的程序关闭。

use std::path::PathBuf;

use rusqlite::Connection;

use super::{SCHEMA_KEY, Store};
use crate::{
    ConversationId, LedgerNoteKind, ModelChoice, NewLedgerNote, NewReport, NewTask, ReportOrigin,
    ReportState, TaskError, TaskPolicy,
};

const T0: i64 = 1_700_000_000_000;

fn draft(title: &str) -> NewTask {
    NewTask {
        title: title.to_owned(),
        requirement: "读官方文档，给出能操作的例子".to_owned(),
        dod: "给出例子和出处".to_owned(),
        cwd: PathBuf::from("/tmp/kind-cwd"),
        conversation: ConversationId::new("kind-session-1"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-chat".to_owned(),
            effort_wanted: "default".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn open_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn schema_version(store: &Store) -> String {
    let conn = store.lock().unwrap();
    conn.query_row("SELECT value FROM kv WHERE key = ?1", [SCHEMA_KEY], |row| {
        row.get(0)
    })
    .unwrap()
}

#[test]
fn migrates_v1_database_to_v2_and_keeps_old_tasks_without_constraints() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let task_id = {
        let store = Store::open(&path).unwrap();
        let task = store.create_task(draft("旧任务"), T0).unwrap();
        task.id
    };
    // 模拟 v1：去掉账本表、把版本写回 1。
    let conn = Connection::open(&path).unwrap();
    conn.execute_batch(
        "DROP TABLE ledger_note; UPDATE kv SET value = '1' WHERE key = 'schema_version';",
    )
    .unwrap();
    drop(conn);

    let store = Store::open(&path).unwrap();
    assert_eq!(schema_version(&store), "2");
    let task = store.task(&task_id).unwrap().expect("旧任务还在");
    assert!(task.current_constraints().is_empty());
    assert!(store.ledger_notes(&task_id).unwrap().is_empty());
    let conn = store.lock().unwrap();
    let exists: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ledger_note')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(exists);
}

#[test]
fn rejects_databases_older_than_v1() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    {
        Store::open(&path).unwrap();
    }
    let conn = Connection::open(&path).unwrap();
    conn.execute("UPDATE kv SET value = '0' WHERE key = ?1", [SCHEMA_KEY])
        .unwrap();
    drop(conn);
    assert!(matches!(
        Store::open(&path),
        Err(TaskError::SchemaUnsupported { found: 0, .. })
    ));
}

#[test]
fn constraints_live_on_requirement_versions_and_carry_forward() {
    let (_directory, store) = open_store();
    let task = store
        .create_task_with_constraints(
            draft("带约束"),
            TaskPolicy::default(),
            vec!["别加新依赖".to_owned()],
            T0,
        )
        .unwrap();
    assert_eq!(task.current_constraints(), ["别加新依赖"]);
    // 先完成 launch，避免「排队时不能改要求」。
    let claim = store
        .claim_effect(crate::EffectKind::Launch, T0)
        .unwrap()
        .expect("launch 在排队");
    store.complete_effect(&claim).unwrap();

    let updated = store
        .update_requirement_with_constraints(&task.id, "改一下要求", None, None, T0 + 1)
        .unwrap();
    assert_eq!(updated.current_constraints(), ["别加新依赖"]);
    assert_eq!(updated.versions.len(), 2);

    let changed = store
        .update_requirement_with_constraints(
            &task.id,
            "再改一下要求",
            None,
            Some(vec!["只改一个文件".to_owned()]),
            T0 + 2,
        )
        .unwrap();
    assert_eq!(changed.current_constraints(), ["只改一个文件"]);
    let scoped = store
        .update_write_scope(&task.id, "out.txt", T0 + 3)
        .unwrap();
    assert_eq!(scoped.current_constraints(), ["只改一个文件"]);
}

#[test]
fn same_topic_note_closes_the_old_one() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft("记账本"), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[NewLedgerNote {
                kind: LedgerNoteKind::Rejected,
                topic: "cache".to_owned(),
                text: "先试了缓存".to_owned(),
            }],
            T0 + 2,
        )
        .unwrap();
    let counts = store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[NewLedgerNote {
                kind: LedgerNoteKind::Rejected,
                topic: "cache".to_owned(),
                text: "缓存不可行".to_owned(),
            }],
            T0 + 3,
        )
        .unwrap();
    assert_eq!((counts.rejected, counts.next), (1, 0));
    let notes = store.ledger_notes(&task.id).unwrap();
    assert_eq!(notes.len(), 2);
    let old = notes.iter().find(|note| note.text == "先试了缓存").unwrap();
    assert_eq!(old.closed_why.as_deref(), Some("被新的一条替换"));
    assert!(old.closed_at.is_some());
    let current = notes.iter().find(|note| note.text == "缓存不可行").unwrap();
    assert!(current.closed_at.is_none());
}

#[test]
fn ready_report_closes_next_notes() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft("下一步"), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[NewLedgerNote {
                kind: LedgerNoteKind::Next,
                topic: "step".to_owned(),
                text: "先跑测试".to_owned(),
            }],
            T0 + 2,
        )
        .unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "改完了".to_owned(),
                question: None,
            },
            T0 + 3,
        )
        .unwrap();
    let notes = store.ledger_notes(&task.id).unwrap();
    assert_eq!(notes[0].closed_why.as_deref(), Some("第 1 次交了结果"));
}

#[test]
fn accepting_closes_all_notes() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft("接受关账本"), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .add_ledger_notes(
            &task.id,
            attempt,
            &[
                NewLedgerNote {
                    kind: LedgerNoteKind::Rejected,
                    topic: "a".to_owned(),
                    text: "这条否掉".to_owned(),
                },
                NewLedgerNote {
                    kind: LedgerNoteKind::Next,
                    topic: "b".to_owned(),
                    text: "下一步".to_owned(),
                },
            ],
            T0 + 2,
        )
        .unwrap();
    let report = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "改完了".to_owned(),
                question: None,
            },
            T0 + 3,
        )
        .unwrap();
    let acceptance = store.accept(&task.id, &report, T0 + 4).unwrap();
    assert_eq!(acceptance.report_id, report);
    let notes = store.ledger_notes(&task.id).unwrap();
    assert!(notes.iter().all(|note| note.closed_at.is_some()));
    let rejected = notes
        .iter()
        .find(|note| note.kind == LedgerNoteKind::Rejected)
        .unwrap();
    assert_eq!(rejected.closed_why.as_deref(), Some("任务已关闭"));
}

#[test]
fn rejects_ledger_notes_that_are_too_long_or_on_a_gate() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft("账本拒绝"), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    assert!(matches!(
        store
            .add_ledger_notes(
                &task.id,
                attempt,
                &[NewLedgerNote {
                    kind: LedgerNoteKind::Next,
                    topic: "x".to_owned(),
                    text: "长".repeat(401),
                }],
                T0 + 2,
            )
            .unwrap_err(),
        TaskError::LedgerTextTooLong(401)
    ));
}
