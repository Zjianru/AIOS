//! 阶段表逐行：只验 T024.1 可达的值和 spec 的行序。

use std::path::PathBuf;

use super::Store;
use crate::{
    AttemptEnd, ConversationId, EffectKind, MissingItem, ModelChoice, NewReport, NewTask,
    ReportState, Stage, Task,
};

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn create(store: &Store) -> Task {
    store
        .create_task(
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
            },
            T0,
        )
        .unwrap()
}

fn finish_launch(store: &Store) {
    let launch = store
        .claim_effect(EffectKind::Launch, T0)
        .unwrap()
        .expect("launch 还在排队");
    store.complete_effect(&launch).unwrap();
}

fn record(store: &Store, task: &Task, state: ReportState) {
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state,
                summary: "报告".to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
}

#[test]
fn stage_needs_conditions_when_the_package_is_missing() {
    let (_directory, store) = open_store();
    let task = create(&store);
    let missing = vec![MissingItem::new("deepseek-flash 的凭证").unwrap()];
    store
        .record_package_check(&task.id, missing.clone())
        .unwrap();

    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    assert_eq!(
        store.task(&task.id).unwrap().unwrap().missing,
        missing,
        "执行者写什么缺项，任务里就存什么"
    );
}

#[test]
fn stage_needs_conditions_after_a_blocked_report() {
    let (_directory, store) = open_store();
    let task = create(&store);
    record(&store, &task, ReportState::Blocked);

    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
}

#[test]
fn stage_needs_conditions_after_a_failed_report() {
    let (_directory, store) = open_store();
    let task = create(&store);
    record(&store, &task, ReportState::Failed);

    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
}

#[test]
fn stage_is_ready_for_review_after_a_ready_report() {
    let (_directory, store) = open_store();
    let task = create(&store);
    record(&store, &task, ReportState::Ready);

    assert_eq!(store.stage(&task.id).unwrap(), Stage::ReadyForReview);
}

#[test]
fn stage_is_pending_without_an_attempt() {
    let (_directory, store) = open_store();
    let task = create(&store);
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);

    finish_launch(&store);
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);
}

#[test]
fn stage_is_pending_while_launch_is_queued_even_with_an_attempt() {
    let (_directory, store) = open_store();
    let task = create(&store);
    store.begin_attempt(&task.id, T0 + 1).unwrap();

    assert_eq!(store.stage(&task.id).unwrap(), Stage::Pending);
}

#[test]
fn stage_is_running_after_launch_completes_and_an_attempt_starts() {
    let (_directory, store) = open_store();
    let task = create(&store);
    finish_launch(&store);
    store.begin_attempt(&task.id, T0 + 1).unwrap();

    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
}

#[test]
fn stage_is_running_while_the_report_is_waiting() {
    let (_directory, store) = open_store();
    let task = create(&store);
    finish_launch(&store);
    record(&store, &task, ReportState::Waiting);

    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
}

#[test]
fn stage_returns_to_running_when_the_package_is_complete() {
    let (_directory, store) = open_store();
    let task = create(&store);
    store
        .record_package_check(&task.id, vec![MissingItem::new("Exa 的凭证").unwrap()])
        .unwrap();
    store.record_package_check(&task.id, Vec::new()).unwrap();
    finish_launch(&store);
    store.begin_attempt(&task.id, T0 + 1).unwrap();

    assert_eq!(store.stage(&task.id).unwrap(), Stage::Running);
}

#[test]
fn stage_ignores_a_late_report_from_an_older_attempt() {
    let (_directory, store) = open_store();
    let task = create(&store);
    finish_launch(&store);
    let first = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .end_attempt(&task.id, first, T0 + 2, AttemptEnd::Ended)
        .unwrap();
    let second = store.begin_attempt(&task.id, T0 + 3).unwrap();
    store
        .record_report(
            &task.id,
            first,
            NewReport {
                state: ReportState::Ready,
                summary: "旧 Attempt 的迟到报告".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();

    assert_eq!(
        store.stage(&task.id).unwrap(),
        Stage::Running,
        "最新 Attempt {} 没有报告，旧报告不能把阶段推到待检查",
        second
    );
}

#[test]
fn stage_needs_conditions_when_missing_outranks_a_ready_report() {
    let (_directory, store) = open_store();
    let task = create(&store);
    record(&store, &task, ReportState::Ready);
    store
        .record_package_check(
            &task.id,
            vec![MissingItem::new("模型选择缺 deepseek-chat").unwrap()],
        )
        .unwrap();

    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
}
