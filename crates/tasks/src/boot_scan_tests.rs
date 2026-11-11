//! 启动扫描：上次没结束的 Attempt 这次启动怎么收尾、restart 效果怎么合并。

use std::path::PathBuf;

use super::Store;
use crate::{
    Attempt, AttemptEnd, AttemptId, BootId, BootScan, ConversationId, EffectKind, EffectPayload,
    ModelChoice, NewReport, NewTask, ReportOrigin, ReportState, Stage, Task,
};

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn create(store: &Store, title: &str) -> Task {
    store
        .create_task(
            NewTask {
                title: title.to_owned(),
                requirement: "读官方文档，给出能操作的例子".to_owned(),
                dod: "给出例子和出处".to_owned(),
                cwd: PathBuf::from("/tmp/kind-cwd"),
                conversation: ConversationId::new(format!("kind-for-{title}")),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-chat".to_owned(),
                    effort_wanted: "default".to_owned(),
                },
                tools: vec!["read".to_owned()],
            },
            T0,
        )
        .unwrap()
}

/// 崩溃前真的开出过会话的 Attempt。
fn open_attempt(store: &Store, task: &Task) -> AttemptId {
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .attach_session(
            &task.id,
            attempt,
            ConversationId::new(format!("thin-for-{}", task.id)),
        )
        .unwrap();
    attempt
}

fn report(
    store: &Store,
    task: &Task,
    attempt: AttemptId,
    state: ReportState,
    question: Option<&str>,
    at: i64,
) {
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state,
                origin: ReportOrigin::Agent,
                summary: "这次 Attempt 的报告".to_owned(),
                question: question.map(str::to_owned),
            },
            at,
        )
        .unwrap();
}

fn attempt_row(store: &Store, task: &Task, attempt: AttemptId) -> Attempt {
    store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .attempt(attempt)
        .cloned()
        .expect("Attempt 还在")
}

#[test]
fn a_running_attempt_without_a_report_is_interrupted_with_one_restart() {
    let (_directory, store) = open_store();
    let task = create(&store, "在跑没报");
    let attempt = open_attempt(&store, &task);

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(scan.interrupted, vec![task.id.clone()]);
    let row = attempt_row(&store, &task, attempt);
    assert_eq!(row.end, Some(AttemptEnd::Interrupted));
    assert_eq!(row.ended_at, Some(T0 + 10));
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::NeedsConditions,
        "最近 Attempt 被打断且没有新 Attempt → 待补足条件（被打断了）"
    );

    let restart = store
        .claim_effect(EffectKind::Restart, T0 + 10)
        .unwrap()
        .expect("要排一条 restart");
    assert_eq!(
        restart.payload,
        EffectPayload::Restart {
            boot_id: BootId::new("boot-1"),
            task_ids: vec![task.id.clone()],
        }
    );
    assert_eq!(restart.key, "boot-1");
    assert!(store.complete_effect(&restart).unwrap());
    assert_eq!(
        store.claim_effect(EffectKind::Restart, T0 + 10).unwrap(),
        None,
        "同一次启动只排一条 restart"
    );
}

#[test]
fn a_report_with_no_new_input_ends_the_attempt_reported() {
    for state in [
        ReportState::Ready,
        ReportState::Blocked,
        ReportState::Failed,
        ReportState::Waiting,
    ] {
        let (_directory, store) = open_store();
        let task = create(&store, "报了没输入");
        let attempt = open_attempt(&store, &task);
        report(&store, &task, attempt, state, None, T0 + 2);

        let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

        assert_eq!(scan.reported, vec![task.id.clone()], "{state:?}");
        assert_eq!(
            attempt_row(&store, &task, attempt).end,
            Some(AttemptEnd::Reported),
            "{state:?} 的报告之后没有新输入，这次 Attempt 已经交了结果"
        );
        assert_eq!(
            store.claim_effect(EffectKind::Restart, T0 + 10).unwrap(),
            None,
            "{state:?} 不该排 restart"
        );
    }
}

#[test]
fn an_input_after_the_report_makes_the_attempt_interrupted() {
    let (_directory, store) = open_store();
    let task = create(&store, "报了又输入");
    let attempt = open_attempt(&store, &task);
    report(&store, &task, attempt, ReportState::Ready, None, T0 + 2);
    let said = store.say(&task.id, "接着做", T0 + 3).unwrap();
    assert_eq!(said.new_attempt, None, "Attempt 还没结束，不另开一次");

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(scan.interrupted, vec![task.id.clone()]);
    assert_eq!(
        attempt_row(&store, &task, attempt).end,
        Some(AttemptEnd::Interrupted)
    );
    assert!(
        store
            .claim_effect(EffectKind::Restart, T0 + 10)
            .unwrap()
            .is_some(),
        "报告之后又有了输入，要排 restart"
    );
    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::NeedsConditions,
        "打断排在待检查前面"
    );
}

#[test]
fn an_attempt_that_never_opened_is_closed_not_opened() {
    let (_directory, store) = open_store();
    let task = create(&store, "没开成");
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(scan.not_opened, vec![task.id.clone()]);
    assert!(scan.interrupted.is_empty());
    let row = attempt_row(&store, &task, attempt);
    assert_eq!(row.end, Some(AttemptEnd::NotOpened));
    assert_eq!(row.ended_at, Some(T0 + 10));
    assert_eq!(
        store.claim_effect(EffectKind::Restart, T0 + 10).unwrap(),
        None,
        "没开成的 Attempt 不算打断"
    );
}

#[test]
fn the_question_on_the_latest_report_is_kept() {
    let (_directory, store) = open_store();
    let task = create(&store, "等回答");
    let attempt = open_attempt(&store, &task);
    report(
        &store,
        &task,
        attempt,
        ReportState::Blocked,
        Some("先跑哪个分支？"),
        T0 + 2,
    );

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(scan.reported, vec![task.id.clone()]);
    let loaded = store.task(&task.id).unwrap().unwrap();
    let latest = loaded.latest_report().expect("报告还在");
    assert_eq!(latest.state, ReportState::Blocked);
    assert_eq!(latest.summary, "这次 Attempt 的报告");
    assert_eq!(
        latest.question.as_deref(),
        Some("先跑哪个分支？"),
        "待答的问题原样保留"
    );
}

#[test]
fn one_boot_queues_a_single_restart_for_all_interrupted_tasks() {
    let (_directory, store) = open_store();
    let first = create(&store, "甲");
    let second = create(&store, "乙");
    let untouched = create(&store, "没开跑");
    open_attempt(&store, &first);
    open_attempt(&store, &second);

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(scan.interrupted.len(), 2);
    assert!(scan.interrupted.contains(&first.id));
    assert!(scan.interrupted.contains(&second.id));
    assert!(!scan.interrupted.contains(&untouched.id));
    assert_eq!(
        store.effect_health(EffectKind::Restart).unwrap().waiting,
        1,
        "同一次启动的多个任务合成一条 restart（键是 bootId）"
    );
    let restart = store
        .claim_effect(EffectKind::Restart, T0 + 10)
        .unwrap()
        .expect("要有一条 restart");
    let EffectPayload::Restart { task_ids, .. } = &restart.payload else {
        panic!("restart 载荷不是 restart");
    };
    assert_eq!(task_ids.len(), 2, "{task_ids:?}");
    assert!(task_ids.contains(&first.id));
    assert!(task_ids.contains(&second.id));
    assert!(store.complete_effect(&restart).unwrap());
    assert_eq!(
        store.claim_effect(EffectKind::Restart, T0 + 10).unwrap(),
        None,
        "同一次启动只排一条 restart"
    );
}

#[test]
fn a_second_boot_queues_only_the_tasks_it_interrupted() {
    let (_directory, store) = open_store();
    let first = create(&store, "甲");
    let second = create(&store, "乙");
    open_attempt(&store, &first);
    store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    open_attempt(&store, &second);
    store.scan_at_boot(&BootId::new("boot-2"), T0 + 20).unwrap();

    let first_restart = store
        .claim_effect(EffectKind::Restart, T0 + 20)
        .unwrap()
        .expect("第一次启动的 restart");
    assert_eq!(first_restart.key, "boot-1");
    assert_eq!(
        first_restart.payload,
        EffectPayload::Restart {
            boot_id: BootId::new("boot-1"),
            task_ids: vec![first.id.clone()],
        },
        "第一次启动只记自己打断的甲"
    );
    assert!(store.complete_effect(&first_restart).unwrap());

    let second_restart = store
        .claim_effect(EffectKind::Restart, T0 + 20)
        .unwrap()
        .expect("第二次启动的 restart");
    assert_eq!(second_restart.key, "boot-2");
    assert_eq!(
        second_restart.payload,
        EffectPayload::Restart {
            boot_id: BootId::new("boot-2"),
            task_ids: vec![second.id.clone()],
        },
        "第二次启动只记自己打断的乙，不带上甲"
    );
    assert!(store.complete_effect(&second_restart).unwrap());
}

#[test]
fn an_unsent_input_before_the_report_keeps_its_delivery_effect() {
    let (_directory, store) = open_store();
    let task = create(&store, "有没送出的输入");
    let attempt = open_attempt(&store, &task);
    let said = store.say(&task.id, "别忘了看日志", T0 + 2).unwrap();
    assert_eq!(said.new_attempt, None, "在跑时输入不另开 Attempt");
    report(&store, &task, attempt, ReportState::Ready, None, T0 + 3);

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(scan.reported, vec![task.id.clone()]);
    assert_eq!(
        attempt_row(&store, &task, attempt).end,
        Some(AttemptEnd::Reported)
    );
    assert!(
        store
            .effect_pending(EffectKind::Delivery, task.id.as_str())
            .unwrap(),
        "没送出的输入留给 T028 的 delivery 补送"
    );
}

#[test]
fn an_attempt_without_an_end_value_reads_as_unfinished() {
    let (_directory, store) = open_store();
    let task = create(&store, "旧库没有 end");
    let attempt = open_attempt(&store, &task);
    store
        .end_attempt(&task.id, attempt, T0 + 2, AttemptEnd::Reported)
        .unwrap();
    store
        .conn
        .lock()
        .unwrap()
        .execute(
            "UPDATE task SET value = replace(value, '\"end\":\"reported\"', '\"end\":null')
             WHERE id = ?1",
            [task.id.as_str()],
        )
        .unwrap();

    let scan = store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    assert_eq!(
        scan.interrupted,
        vec![task.id.clone()],
        "旧库里没有 end 的 Attempt 按没结束读"
    );
    assert_eq!(
        attempt_row(&store, &task, attempt).end,
        Some(AttemptEnd::Interrupted)
    );
}

#[test]
fn a_second_boot_leaves_a_finished_attempt_alone() {
    let (_directory, store) = open_store();
    let task = create(&store, "第一次就收过了");
    open_attempt(&store, &task);
    store.scan_at_boot(&BootId::new("boot-1"), T0 + 10).unwrap();

    let second = store.scan_at_boot(&BootId::new("boot-2"), T0 + 20).unwrap();

    assert_eq!(second, BootScan::default(), "收过尾的 Attempt 不再算没结束");
    let restart = store
        .claim_effect(EffectKind::Restart, T0 + 20)
        .unwrap()
        .expect("第一次启动排的 restart 还在");
    assert_eq!(restart.key, "boot-1");
    assert!(store.complete_effect(&restart).unwrap());
    assert_eq!(
        store.claim_effect(EffectKind::Restart, T0 + 20).unwrap(),
        None,
        "第二次启动不另排 restart"
    );
}
