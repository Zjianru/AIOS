//! 任务库的存储边界：建任务和 launch 同一事务、回滚、重开、schema、WAL。

use std::path::PathBuf;

use rusqlite::Connection;

use super::{SCHEMA_KEY, Store};
use crate::{
    ConversationId, EffectKind, EffectPayload, ModelChoice, NewTask, Task, TaskError, TaskId,
};

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn draft() -> NewTask {
    NewTask {
        title: "查一下 exa 怎么用".to_owned(),
        requirement: "读官方文档，给出能操作的例子".to_owned(),
        dod: "给出例子和出处".to_owned(),
        cwd: PathBuf::from("/tmp/kind-cwd"),
        conversation: ConversationId::new("kind-session-1"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-chat".to_owned(),
            effort_wanted: "default".to_owned(),
        },
        tools: vec!["read".to_owned(), "grep".to_owned()],
    }
}

#[test]
fn creates_a_task_and_queues_launch_together() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft(), T0).unwrap();

    assert_eq!(task.title, "查一下 exa 怎么用");
    assert_eq!(task.created_at, T0);
    assert_eq!(task.conversation.as_str(), "kind-session-1");
    assert_eq!(task.model.model, "deepseek-chat");
    assert!(task.writes.is_empty());
    assert!(!task.code_mode);
    assert_eq!(store.task(&task.id).unwrap(), Some(task.clone()));

    let launch = store.claim_effect(EffectKind::Launch, T0).unwrap();
    assert_eq!(
        launch.map(|claim| claim.payload),
        Some(EffectPayload::Launch {
            task_id: task.id.clone()
        })
    );
}

#[test]
fn creates_distinct_task_ids() {
    let (_directory, store) = open_store();
    let first = store.create_task(draft(), T0).unwrap();
    let second = store.create_task(draft(), T0).unwrap();

    assert!(!first.id.as_str().is_empty());
    assert_ne!(first.id, second.id);
}

#[test]
fn rolls_back_the_task_when_its_launch_effect_cannot_be_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let store = Store::open(&path).unwrap();
    let other = Connection::open(&path).unwrap();
    other
        .execute_batch(
            "CREATE TRIGGER fail_effect BEFORE INSERT ON effect
             BEGIN SELECT RAISE(ABORT, 'effect write failed'); END;",
        )
        .unwrap();

    let result = store.create_task(draft(), T0);
    assert!(
        matches!(result, Err(TaskError::Sqlite(_))),
        "effect 写失败要让建任务一起回滚: {result:?}"
    );
    let tasks: i64 = store
        .conn
        .lock()
        .unwrap()
        .query_row("SELECT COUNT(*) FROM task", [], |row| row.get(0))
        .unwrap();
    assert_eq!(tasks, 0);
    assert_eq!(store.claim_effect(EffectKind::Launch, T0).unwrap(), None);
}

#[test]
fn reopens_with_the_task_and_queued_launch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let task = {
        let store = Store::open(&path).unwrap();
        store.create_task(draft(), T0).unwrap()
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.task(&task.id).unwrap(), Some(task.clone()));
    assert_eq!(
        store
            .claim_effect(EffectKind::Launch, T0)
            .unwrap()
            .map(|claim| claim.payload),
        Some(EffectPayload::Launch { task_id: task.id })
    );
}

#[test]
fn refuses_a_database_written_by_a_newer_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch("CREATE TABLE kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);")
        .unwrap();
    connection
        .execute(
            "INSERT INTO kv (key, value) VALUES (?1, ?2)",
            [SCHEMA_KEY, "2"],
        )
        .unwrap();
    drop(connection);

    assert!(matches!(
        Store::open(&path),
        Err(TaskError::SchemaTooNew {
            found: 2,
            supported: 1
        })
    ));
}

#[test]
fn uses_wal_and_full_synchronous() {
    let (_directory, store) = open_store();
    let connection = store.conn.lock().unwrap();
    let journal: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    let synchronous: i64 = connection
        .query_row("PRAGMA synchronous", [], |row| row.get(0))
        .unwrap();

    assert_eq!(journal.to_ascii_lowercase(), "wal");
    assert_eq!(synchronous, 2, "2 = FULL");
}

#[test]
fn rejects_a_title_outside_one_to_two_hundred_chars() {
    let (_directory, store) = open_store();
    let mut blank = draft();
    blank.title = String::new();
    assert!(matches!(
        store.create_task(blank, T0),
        Err(TaskError::InvalidTitle(0))
    ));

    let mut at_limit = draft();
    at_limit.title = "字".repeat(200);
    assert!(store.create_task(at_limit, T0).is_ok());

    let mut too_long = draft();
    too_long.title = "字".repeat(201);
    assert!(matches!(
        store.create_task(too_long, T0),
        Err(TaskError::InvalidTitle(201))
    ));
}

#[test]
fn store_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Store>();
}

#[test]
fn rejects_empty_requirement_dod_and_conversation() {
    let (_directory, store) = open_store();
    let mut requirement = draft();
    requirement.requirement = "  ".to_owned();
    assert!(matches!(
        store.create_task(requirement, T0),
        Err(TaskError::EmptyRequirement)
    ));

    let mut dod = draft();
    dod.dod = String::new();
    assert!(matches!(
        store.create_task(dod, T0),
        Err(TaskError::EmptyDod)
    ));

    let mut conversation = draft();
    conversation.conversation = ConversationId::new("");
    assert!(matches!(
        store.create_task(conversation, T0),
        Err(TaskError::EmptyConversation)
    ));
}

#[test]
fn task_id_is_stable_across_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let id: TaskId = {
        let store = Store::open(&path).unwrap();
        store.create_task(draft(), T0).unwrap().id
    };
    let store = Store::open(&path).unwrap();
    let loaded: Option<Task> = store.task(&id).unwrap();
    assert!(loaded.is_some());
}
