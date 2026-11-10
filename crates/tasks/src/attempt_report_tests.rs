//! Attempt 生命周期、报告与 relay 的事务边界。

use std::path::PathBuf;

use rusqlite::Connection;

use super::Store;
use crate::{
    AttemptEnd, AttemptId, ConversationId, EffectKind, EffectPayload, ModelChoice, NewReport,
    NewTask, ReportOrigin, ReportState, Task, TaskError,
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

fn report(state: ReportState, summary: &str) -> NewReport {
    NewReport {
        state,
        // waiting 只有执行者会合成；其余报告在存储边界的单测里按 ThinAgent 的报告取。
        origin: if state == ReportState::Waiting {
            ReportOrigin::Executor
        } else {
            ReportOrigin::Agent
        },
        summary: summary.to_owned(),
        question: None,
    }
}

#[test]
fn starts_an_attempt_and_writes_back_its_session() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft(), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();

    let loaded = store.task(&task.id).unwrap();
    assert_eq!(loaded.as_ref().map(|task| task.runs.len()), Some(1));
    assert_eq!(
        loaded
            .as_ref()
            .map(|task| task.runs[0].attempts[0].started_at),
        Some(T0 + 1)
    );
    assert_eq!(
        loaded
            .as_ref()
            .map(|task| task.runs[0].attempts[0].conversation.clone()),
        Some(None)
    );

    store
        .attach_session(&task.id, attempt, ConversationId::new("thin-session-1"))
        .unwrap();
    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        loaded.runs[0].attempts[0]
            .conversation
            .as_ref()
            .map(ConversationId::as_str),
        Some("thin-session-1")
    );
}

#[test]
fn refuses_a_second_attempt_while_one_is_open() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft(), T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();

    assert!(
        matches!(
            store.begin_attempt(&task.id, T0 + 2),
            Err(TaskError::AttemptStillOpen { attempt: open, .. }) if open == attempt.0
        ),
        "同一个任务不能同时有两个开着的 Attempt"
    );
}

#[test]
fn closes_attempts_that_never_got_a_session_after_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let (task_id, attempt) = {
        let store = Store::open(&path).unwrap();
        let task = store.create_task(draft(), T0).unwrap();
        let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
        (task.id, attempt)
    };

    let store = Store::open(&path).unwrap();
    assert_eq!(store.close_unopened_attempts(&task_id, T0 + 2).unwrap(), 1);
    let loaded = store.task(&task_id).unwrap().unwrap();
    let attempt_row = loaded.attempt(attempt).expect("Attempt 还在");
    assert_eq!(attempt_row.ended_at, Some(T0 + 2));
    assert_eq!(attempt_row.end, Some(AttemptEnd::NotOpened));
    assert_eq!(store.close_unopened_attempts(&task_id, T0 + 3).unwrap(), 0);
}

#[test]
fn keeps_an_attempt_with_a_session_open_when_closing_unopened() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);
    store
        .attach_session(&task.id, attempt, ConversationId::new("thin-session-1"))
        .unwrap();

    assert_eq!(store.close_unopened_attempts(&task.id, T0 + 2).unwrap(), 0);
    let loaded = store.task(&task.id).unwrap().unwrap();
    let attempt_row = loaded.attempt(attempt).expect("Attempt 还在");
    assert_eq!(attempt_row.ended_at, None);
    assert_eq!(attempt_row.end, None);
}

#[test]
fn records_a_report_and_queues_relay_in_one_transaction() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);

    let report_id = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Blocked,
                origin: ReportOrigin::Package,
                summary: "缺 Exa key".to_owned(),
                question: Some("要我去设置里填吗？".to_owned()),
            },
            T0 + 2,
        )
        .unwrap();

    assert!(!report_id.as_str().is_empty());
    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.reports.len(), 1);
    assert_eq!(loaded.reports[0].state, ReportState::Blocked);
    assert_eq!(
        loaded.reports[0].origin,
        ReportOrigin::Package,
        "报告来源要随报告一起落库"
    );
    assert_eq!(loaded.reports[0].attempt_id, attempt);
    assert_eq!(loaded.reports[0].at, T0 + 2);
    assert_eq!(
        loaded.latest_report().map(|report| report.state),
        Some(ReportState::Blocked)
    );

    let relay = store.claim_effect(EffectKind::Relay, T0 + 2).unwrap();
    assert_eq!(
        relay.map(|claim| (claim.key, claim.payload)),
        Some((
            report_id.to_string(),
            EffectPayload::Relay {
                task_id: task.id.clone(),
                report_id: report_id.clone(),
            }
        ))
    );
}

#[test]
fn rolls_back_the_report_when_its_relay_effect_cannot_be_written() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("tasks.sqlite");
    let store = Store::open(&path).unwrap();
    let (task, attempt) = task_with_attempt(&store);
    let other = Connection::open(&path).unwrap();
    other
        .execute_batch(
            "CREATE TRIGGER fail_effect BEFORE INSERT ON effect
             BEGIN SELECT RAISE(ABORT, 'effect write failed'); END;",
        )
        .unwrap();

    let result = store.record_report(
        &task.id,
        attempt,
        report(ReportState::Ready, "有结果了"),
        T0 + 2,
    );
    assert!(
        matches!(result, Err(TaskError::Sqlite(_))),
        "relay 写失败要让报告一起回滚: {result:?}"
    );
    let loaded = store.task(&task.id).unwrap().unwrap();
    assert!(loaded.reports.is_empty());
    assert_eq!(store.claim_effect(EffectKind::Relay, T0 + 2).unwrap(), None);
}

#[test]
fn repeating_the_same_terminal_report_returns_the_first_report() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);
    let first = store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "有结果了"),
            T0 + 2,
        )
        .unwrap();
    let repeated = store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "有结果了（重发）"),
            T0 + 3,
        )
        .unwrap();

    assert_eq!(repeated, first);
    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.reports.len(), 1);
    assert_eq!(
        store
            .claim_effect(EffectKind::Relay, T0 + 3)
            .unwrap()
            .map(|claim| claim.key),
        Some(first.to_string())
    );
}

#[test]
fn a_ready_report_replaces_a_synthesized_waiting_report() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);
    let waiting = store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Waiting, "turn 结束，还没上报"),
            T0 + 2,
        )
        .unwrap();
    let ready = store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "有结果了"),
            T0 + 3,
        )
        .unwrap();
    let repeated = store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "有结果了（重发）"),
            T0 + 4,
        )
        .unwrap();

    assert_ne!(
        ready, waiting,
        "终态要自己一条 relay（request id 带报告 id），不能复用 waiting 的报告 id"
    );
    assert_eq!(repeated, ready, "同终态幂等，返回该终态的报告 id");
    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(loaded.reports.len(), 2, "合成的 waiting 留作历史");
    assert_eq!(
        loaded.latest_report().map(|report| report.state),
        Some(ReportState::Ready)
    );
    assert!(
        store
            .effect_pending(EffectKind::Relay, ready.as_str())
            .unwrap(),
        "终态报告要排 relay"
    );
}

#[test]
fn refuses_a_different_terminal_report_for_the_same_attempt() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);
    store
        .record_report(
            &task.id,
            attempt,
            report(ReportState::Failed, "失败了"),
            T0 + 2,
        )
        .unwrap();

    assert!(matches!(
        store.record_report(
            &task.id,
            attempt,
            report(ReportState::Ready, "突然又好了"),
            T0 + 3
        ),
        Err(TaskError::ReportConflict {
            attempt: conflict,
            existing: ReportState::Failed,
            requested: ReportState::Ready,
        }) if conflict == attempt.0
    ));
}

#[test]
fn latest_report_uses_the_latest_attempt_only() {
    let (_directory, store) = open_store();
    let task = store.create_task(draft(), T0).unwrap();
    let first = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .end_attempt(&task.id, first, T0 + 2, AttemptEnd::Ended)
        .unwrap();
    let second = store.begin_attempt(&task.id, T0 + 3).unwrap();

    store
        .record_report(
            &task.id,
            first,
            report(ReportState::Ready, "旧报告"),
            T0 + 4,
        )
        .unwrap();
    store
        .record_report(
            &task.id,
            second,
            report(ReportState::Failed, "新报告"),
            T0 + 5,
        )
        .unwrap();

    let loaded = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        loaded.latest_report().map(|report| report.state),
        Some(ReportState::Failed),
        "旧 Attempt 的迟到报告不能盖过最新 Attempt"
    );
}

#[test]
fn rejects_invalid_report_fields() {
    let (_directory, store) = open_store();
    let (task, attempt) = task_with_attempt(&store);

    assert!(matches!(
        store.record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: " ".to_owned(),
                question: None,
            },
            T0 + 2,
        ),
        Err(TaskError::EmptySummary)
    ));
    assert!(matches!(
        store.record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Blocked,
                origin: ReportOrigin::Agent,
                summary: "缺条件".to_owned(),
                question: Some(" ".to_owned()),
            },
            T0 + 2,
        ),
        Err(TaskError::EmptyQuestion)
    ));
    assert!(matches!(
        store.record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Blocked,
                origin: ReportOrigin::Agent,
                summary: "缺条件".to_owned(),
                question: Some("问".repeat(1001)),
            },
            T0 + 2,
        ),
        Err(TaskError::QuestionTooLong(1001))
    ));
}

#[test]
fn blocked_and_failed_also_replace_a_synthesized_waiting_report() {
    for state in [ReportState::Blocked, ReportState::Failed] {
        let (_directory, store) = open_store();
        let (task, attempt) = task_with_attempt(&store);
        let waiting = store
            .record_report(
                &task.id,
                attempt,
                report(ReportState::Waiting, "turn 结束，还没上报"),
                T0 + 2,
            )
            .unwrap();
        let terminal = store
            .record_report(&task.id, attempt, report(state, "终态"), T0 + 3)
            .unwrap();
        assert_ne!(terminal, waiting, "{state:?} 要自己一条报告");
        let loaded = store.task(&task.id).unwrap().unwrap();
        assert_eq!(
            loaded.latest_report().map(|report| report.state),
            Some(state)
        );
        assert!(
            store
                .effect_pending(EffectKind::Relay, terminal.as_str())
                .unwrap(),
            "{state:?} 要排 relay"
        );
    }
}
