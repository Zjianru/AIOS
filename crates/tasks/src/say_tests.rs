//! 任务对话里的输入：输入、delivery、notice 同事务，和「接着做」的 Attempt。

use std::path::PathBuf;

use rusqlite::Connection;

use super::Store;
use crate::{
    AttemptEnd, AttemptId, AttemptTrigger, ConversationId, EffectKind, EffectPayload, InputId,
    InputSource, ModelChoice, NewReport, NewTask, ReportOrigin, ReportState, SlotKind, Task,
    TaskError, WorkerSlots,
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

/// 开着的 Attempt：用户说话时任务算「在跑」，不另开 Attempt。
fn running_task(store: &Store) -> (Task, AttemptId) {
    let task = store.create_task(draft(), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .attach_session(&task.id, attempt, ConversationId::new("thin-session-1"))
        .unwrap();
    (task, attempt)
}

/// 报过 ready 并收尾的 Attempt：用户说话时要接着开新的 Attempt。
fn reported_task(store: &Store) -> (Task, AttemptId) {
    let (task, attempt) = running_task(store);
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "做完了".to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
    store
        .end_attempt(&task.id, attempt, T0 + 2, AttemptEnd::Reported)
        .unwrap();
    (task, attempt)
}

#[test]
fn records_the_input_delivery_and_notice_in_one_transaction() {
    let (_directory, store) = open_store();
    let (task, _attempt) = running_task(&store);

    let outcome = store.say(&task.id, "别看测试目录", T0 + 10).unwrap();

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.inputs.len(), 1);
    assert_eq!(loaded.inputs[0].id, outcome.input_id);
    assert_eq!(loaded.inputs[0].text, "别看测试目录");
    assert_eq!(loaded.inputs[0].at, T0 + 10);
    assert_eq!(loaded.inputs[0].source, InputSource::User);
    assert_eq!(outcome.new_attempt, None, "在跑时不另开 Attempt");

    let delivery = store
        .claim_effect(EffectKind::Delivery, T0 + 10)
        .unwrap()
        .expect("delivery 应该排上");
    assert_eq!(
        delivery.payload,
        EffectPayload::Delivery {
            task_id: task.id.clone()
        }
    );
    assert_eq!(delivery.key, task.id.as_str());

    let notice = store
        .claim_effect(EffectKind::Notice, T0 + 10)
        .unwrap()
        .expect("notice 应该排上");
    assert_eq!(
        notice.payload,
        EffectPayload::Notice {
            task_id: task.id.clone(),
            input_id: outcome.input_id.clone(),
        }
    );
    assert_eq!(notice.key, format!("cc:{}", outcome.input_id));
}

#[test]
fn rolls_back_the_input_and_wake_when_the_notice_cannot_be_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let store = Store::open(&path).unwrap();
    let (task, _attempt) = reported_task(&store);
    let before = store.task(&task.id).unwrap().unwrap();
    let other = Connection::open(&path).unwrap();
    other
        .execute_batch(
            "CREATE TRIGGER fail_notice BEFORE INSERT ON effect
             WHEN NEW.kind = 'notice'
             BEGIN SELECT RAISE(ABORT, 'notice write failed'); END;",
        )
        .unwrap();

    let result = store.say(&task.id, "接着做吧", T0 + 20);

    assert!(
        matches!(result, Err(TaskError::Sqlite(_))),
        "notice 写失败要让输入、delivery 和新 Attempt 一起回滚: {result:?}"
    );
    assert_eq!(
        store.task(&task.id).unwrap().unwrap(),
        before,
        "输入和新 Attempt 都不能留下"
    );
    assert!(
        !store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap(),
        "delivery 也要一起回滚"
    );
}

#[test]
fn merges_delivery_per_task_and_keeps_one_notice_per_input() {
    let (_directory, store) = open_store();
    let (task, _attempt) = running_task(&store);
    let first = store.say(&task.id, "先看这个", T0 + 10).unwrap();
    let second = store.say(&task.id, "再看那个", T0 + 11).unwrap();

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.inputs.len(), 2);
    assert_eq!(loaded.inputs[1].id, second.input_id);
    assert_eq!(loaded.inputs[1].text, "再看那个");
    assert_eq!(loaded.runs[0].attempts.len(), 1, "在跑时说话不开新 Attempt");

    let delivery = store
        .claim_effect(EffectKind::Delivery, T0 + 11)
        .unwrap()
        .expect("delivery 应该排上");
    assert_eq!(delivery.generation, 1, "两条输入合并成一条 delivery");
    assert_eq!(
        delivery.payload,
        EffectPayload::Delivery {
            task_id: task.id.clone()
        }
    );
    assert!(store.complete_effect(&delivery).unwrap());
    assert_eq!(
        store.claim_effect(EffectKind::Delivery, T0 + 11).unwrap(),
        None
    );

    let first_notice = store
        .claim_effect(EffectKind::Notice, T0 + 11)
        .unwrap()
        .expect("第一条 notice");
    assert_eq!(
        first_notice.payload,
        EffectPayload::Notice {
            task_id: task.id.clone(),
            input_id: first.input_id.clone(),
        }
    );
    assert!(store.complete_effect(&first_notice).unwrap());
    let second_notice = store
        .claim_effect(EffectKind::Notice, T0 + 11)
        .unwrap()
        .expect("第二条 notice");
    assert_eq!(
        second_notice.payload,
        EffectPayload::Notice {
            task_id: task.id.clone(),
            input_id: second.input_id.clone(),
        }
    );
    assert!(store.complete_effect(&second_notice).unwrap());
}

fn wakes(end: Option<AttemptEnd>) -> bool {
    let (_directory, store) = open_store();
    let (task, attempt) = running_task(&store);
    if let Some(end) = end {
        store.end_attempt(&task.id, attempt, T0 + 2, end).unwrap();
    }
    store
        .say(&task.id, "接着做吧", T0 + 3)
        .unwrap()
        .new_attempt
        .is_some()
}

#[test]
fn wakes_only_after_a_finished_attempt() {
    assert!(
        wakes(Some(AttemptEnd::Reported)),
        "reported 之后说话要接着做"
    );
    assert!(wakes(Some(AttemptEnd::Stopped)), "stopped 之后说话要接着做");
    assert!(
        wakes(Some(AttemptEnd::Interrupted)),
        "interrupted 之后说话要接着做"
    );
    assert!(!wakes(None), "还在跑的 Attempt 不另开");
    assert!(
        !wakes(Some(AttemptEnd::NotOpened)),
        "没开成会话的 Attempt 没有会话可接着做"
    );
}

#[test]
fn the_woken_attempt_keeps_the_session_and_records_the_user_message_trigger() {
    let (_directory, store) = open_store();
    let (task, first_attempt) = reported_task(&store);

    let outcome = store.say(&task.id, "接着做吧", T0 + 30).unwrap();

    assert_eq!(outcome.new_attempt, Some(AttemptId(2)));
    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.runs.len(), 1, "新 Attempt 还在同一个 Run 里");
    assert_eq!(loaded.runs[0].attempts.len(), 2);
    assert_eq!(loaded.runs[0].attempts[0].id, first_attempt);
    let second = &loaded.runs[0].attempts[1];
    assert_eq!(second.id, AttemptId(2));
    assert_eq!(
        second.conversation.as_ref().map(ConversationId::as_str),
        Some("thin-session-1"),
        "会话 id 不变"
    );
    assert_eq!(second.trigger, AttemptTrigger::UserMessage);
    assert_eq!(second.started_at, T0 + 30);
    assert_eq!(second.ended_at, None);
    assert_eq!(
        loaded.runs[0].attempts[0].trigger,
        AttemptTrigger::Launch,
        "派发的 Attempt 记 launch"
    );
}

#[test]
fn a_wake_can_take_one_extra_person_seat_when_every_seat_is_full() {
    let (_directory, store) = open_store();
    let (task, _attempt) = reported_task(&store);
    let outcome = store.say(&task.id, "接着做吧", T0 + 30).unwrap();
    assert!(outcome.new_attempt.is_some(), "先叫醒一个停了的任务");

    let mut slots = WorkerSlots::new();
    let running = vec!["another-task".to_owned()];
    slots
        .reserve(task.id.as_str(), SlotKind::Person, &running, 1)
        .expect("席位全满时 person 仍可多占 1 个");
    assert_eq!(slots.used(&running), 2);
}

#[test]
fn reads_an_old_task_row_without_inputs_or_attempt_trigger_as_empty_and_launch() {
    let (_directory, store) = open_store();
    let (task, _attempt) = running_task(&store);
    {
        let conn = store.conn.lock().unwrap();
        let value: String = conn
            .query_row(
                "SELECT value FROM task WHERE id = ?1",
                [task.id.as_str()],
                |row| row.get(0),
            )
            .unwrap();
        let mut json: serde_json::Value = serde_json::from_str(&value).unwrap();
        json.as_object_mut().unwrap().remove("inputs");
        json["runs"][0]["attempts"][0]
            .as_object_mut()
            .unwrap()
            .remove("trigger");
        conn.execute(
            "UPDATE task SET value = ?1 WHERE id = ?2",
            [json.to_string(), task.id.as_str().to_owned()],
        )
        .unwrap();
    }

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert!(loaded.inputs.is_empty(), "旧任务行没有 inputs 按空读");
    assert_eq!(loaded.runs[0].attempts[0].trigger, AttemptTrigger::Launch);
}

#[test]
fn rejects_an_empty_input() {
    let (_directory, store) = open_store();
    let (task, _attempt) = running_task(&store);

    assert!(matches!(
        store.say(&task.id, "  ", T0 + 10),
        Err(TaskError::EmptyInput)
    ));
    assert!(store.task(&task.id).unwrap().unwrap().inputs.is_empty());
}

#[test]
fn a_repeated_notice_merges_under_the_input_key() {
    let (_directory, store) = open_store();
    let notice = EffectPayload::Notice {
        task_id: crate::TaskId::from("task-1"),
        input_id: InputId::from("input-1"),
    };
    store.enqueue_effect(notice.clone(), T0).unwrap();
    store.enqueue_effect(notice, T0 + 1).unwrap();

    let claim = store
        .claim_effect(EffectKind::Notice, T0 + 1)
        .unwrap()
        .expect("notice 应该排上");
    assert_eq!(claim.key, "cc:input-1");
    assert_eq!(claim.generation, 1, "同一个输入 id 的 notice 合并");
    assert_eq!(
        claim.payload,
        EffectPayload::Notice {
            task_id: crate::TaskId::from("task-1"),
            input_id: InputId::from("input-1"),
        }
    );
}
