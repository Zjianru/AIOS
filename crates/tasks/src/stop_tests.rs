//! 立即停的存储边界：一个事务写 stopped 和停止 relay。

use std::path::PathBuf;

use super::{StopAttempt, Store};
use crate::{
    AttemptEnd, AttemptId, ConversationId, EffectKind, EffectPayload, ModelChoice, NewTask, Task,
    TaskId,
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
        tools: vec!["read".to_owned()],
    }
}

fn task_with_attempt(store: &Store) -> (Task, AttemptId) {
    let task = store.create_task(draft(), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    (task, attempt)
}

fn stop_key(task: &TaskId, attempt: AttemptId) -> String {
    format!("stop:{task}:{attempt}")
}

#[test]
fn stopping_records_the_end_and_queues_one_stop_relay() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);

    assert_eq!(
        store.stop_attempt(&task.id, attempt, T0 + 2).unwrap(),
        StopAttempt::Stopped
    );

    let loaded = store.task(&task.id).unwrap().unwrap();
    let row = loaded.attempt(attempt).expect("Attempt 还在");
    assert_eq!(row.end, Some(AttemptEnd::Stopped));
    assert_eq!(row.ended_at, Some(T0 + 2));

    let claim = store
        .claim_effect(EffectKind::Relay, T0 + 2)
        .unwrap()
        .expect("停止 relay 要排上");
    assert_eq!(
        claim.payload,
        EffectPayload::Stop {
            task_id: task.id.clone(),
            attempt_id: attempt,
        }
    );
    assert_eq!(claim.key, stop_key(&task.id, attempt));
}

#[test]
fn stopping_twice_keeps_one_stop_relay() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);

    assert_eq!(
        store.stop_attempt(&task.id, attempt, T0 + 2).unwrap(),
        StopAttempt::Stopped
    );
    assert_eq!(
        store.stop_attempt(&task.id, attempt, T0 + 3).unwrap(),
        StopAttempt::AlreadyStopped
    );

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        loaded.attempt(attempt).unwrap().ended_at,
        Some(T0 + 2),
        "重复停不覆盖第一次的时间"
    );

    let claim = store
        .claim_effect(EffectKind::Relay, T0 + 3)
        .unwrap()
        .expect("第一条停止 relay");
    assert!(store.complete_effect(&claim).unwrap());
    assert_eq!(
        store.claim_effect(EffectKind::Relay, T0 + 4).unwrap(),
        None,
        "重复停不排第二条 relay"
    );
}

#[test]
fn stopping_an_attempt_that_already_ended_changes_nothing() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);
    store
        .end_attempt(&task.id, attempt, T0 + 2, AttemptEnd::Reported)
        .unwrap();

    assert_eq!(
        store.stop_attempt(&task.id, attempt, T0 + 3).unwrap(),
        StopAttempt::AlreadyEnded
    );

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        loaded.attempt(attempt).unwrap().end,
        Some(AttemptEnd::Reported)
    );
    assert!(
        !store
            .effect_pending(EffectKind::Relay, &stop_key(&task.id, attempt))
            .unwrap(),
        "已经结束的 Attempt 不再排停止 relay"
    );
}
