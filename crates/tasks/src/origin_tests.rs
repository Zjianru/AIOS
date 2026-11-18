//! T066.1：任务来源/回合结束事实；表结构不变，旧任务按普通任务读。

use std::path::Path;

use crate::{
    AppointmentOrigin, AppointmentRole, ConversationId, ModelChoice, NewTask, RoundEnd, Stage,
    Store, TaskId,
};

fn create(store: &Store, cwd: &Path, title: &str) -> TaskId {
    store
        .create_task(
            NewTask {
                title: title.to_owned(),
                requirement: "做点事".to_owned(),
                dod: "做完".to_owned(),
                cwd: cwd.to_path_buf(),
                conversation: ConversationId::new("kind-origin-tests"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: Vec::new(),
            },
            1_800_000_000_000,
        )
        .unwrap()
        .id
}

fn origin(role: AppointmentRole) -> AppointmentOrigin {
    AppointmentOrigin {
        appointment_id: "appointment-1".to_owned(),
        run_id: 7,
        role,
    }
}

#[test]
fn old_task_without_origin_reads_as_plain_task() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let id = create(&store, directory.path(), "旧任务");
    let task = store.task(&id).unwrap().unwrap();
    assert_eq!(task.origin, None);
    assert_eq!(task.round_end, None);

    let mut value = serde_json::to_value(&task).unwrap();
    value.as_object_mut().unwrap().remove("origin");
    value.as_object_mut().unwrap().remove("roundEnd");
    let old: crate::Task = serde_json::from_value(value).unwrap();
    assert_eq!(old.origin, None);
    assert_eq!(old.round_end, None);
}

#[test]
fn origin_and_round_end_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let id = create(&store, directory.path(), "触发任务");
    store
        .set_origin(&id, origin(AppointmentRole::Trigger))
        .unwrap();
    store
        .set_round_end(
            &id,
            RoundEnd {
                turn: 3,
                at: 1_800_000_000_123,
            },
        )
        .unwrap();
    let task = store.task(&id).unwrap().unwrap();
    assert_eq!(task.origin.as_ref().unwrap().role, AppointmentRole::Trigger);
    assert_eq!(task.origin.as_ref().unwrap().run_id, 7);
    assert_eq!(
        task.round_end,
        Some(RoundEnd {
            turn: 3,
            at: 1_800_000_000_123
        })
    );
    let json = serde_json::to_string(&task).unwrap();
    let back: crate::Task = serde_json::from_str(&json).unwrap();
    assert_eq!(back.origin, task.origin);
    assert_eq!(back.round_end, task.round_end);
}

#[test]
fn trigger_task_closes_on_round_end_while_plain_task_does_not() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let trigger = create(&store, directory.path(), "约定到点");
    store
        .set_origin(&trigger, origin(AppointmentRole::Trigger))
        .unwrap();
    assert_ne!(
        store.stage(&trigger).unwrap(),
        Stage::Closed,
        "没有 round_end 还在执行"
    );
    store
        .set_round_end(
            &trigger,
            RoundEnd {
                turn: 1,
                at: 1_800_000_000_100,
            },
        )
        .unwrap();
    assert_eq!(store.stage(&trigger).unwrap(), Stage::Closed);

    let plain = create(&store, directory.path(), "普通任务");
    store
        .set_round_end(
            &plain,
            RoundEnd {
                turn: 1,
                at: 1_800_000_000_100,
            },
        )
        .unwrap();
    assert_ne!(
        store.stage(&plain).unwrap(),
        Stage::Closed,
        "普通任务 closed_in 不变"
    );
}

#[test]
fn dispatched_origin_is_serialized_but_does_not_close_on_round_end_alone() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let id = create(&store, directory.path(), "派出去的任务");
    store
        .set_origin(&id, origin(AppointmentRole::Dispatched))
        .unwrap();
    store
        .set_round_end(
            &id,
            RoundEnd {
                turn: 2,
                at: 1_800_000_000_200,
            },
        )
        .unwrap();
    let task = store.task(&id).unwrap().unwrap();
    assert_eq!(
        task.origin.as_ref().unwrap().role,
        AppointmentRole::Dispatched
    );
    assert_ne!(store.stage(&id).unwrap(), Stage::Closed);
}
