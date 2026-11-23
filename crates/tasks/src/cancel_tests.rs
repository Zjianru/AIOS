//! T077.3：取消单项事实、阶段、事务效果和写入口守卫。

use std::path::PathBuf;

use super::*;

const T0: i64 = 1_700_000_000_000;

fn open() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn draft() -> NewTask {
    NewTask {
        title: "查一下取消".to_owned(),
        requirement: "读文档".to_owned(),
        dod: "给出例子".to_owned(),
        cwd: PathBuf::from("/tmp/cancel-cwd"),
        conversation: ConversationId::new("kind-session"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-chat".to_owned(),
            effort_wanted: "default".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn task(store: &Store) -> Task {
    let task = store.create_task(draft(), T0).unwrap();
    // 清掉建任务排的 launch，后面好断言取消效果。
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    task
}

#[test]
fn cancel_is_the_first_stage_fact_and_enqueues_cancel_effect_in_one_transaction() {
    let (_directory, store) = open();
    let task = task(&store);
    let cancelled = store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    assert_eq!(cancelled.at, T0 + 10);
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.cancelled, Some(cancelled));
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    assert_eq!(store.closed_at(&task.id).unwrap(), Some(T0 + 10));
    let claim = store.claim_effect(EffectKind::Cancel, T0 + 10).unwrap();
    assert_eq!(
        claim.map(|claim| claim.payload),
        Some(EffectPayload::Cancel {
            task_id: task.id.clone()
        })
    );
}

#[test]
fn cancelled_task_rejects_every_write_entry_with_the_same_sentence() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .end_attempt(&task.id, attempt, T0 + 2, AttemptEnd::Interrupted)
        .unwrap();
    store
        .cancel(&task.id, CancellationSource::Kind, None, T0 + 10)
        .unwrap();
    let message = CANCELLED_MESSAGE;
    assert_eq!(
        store
            .begin_attempt(&task.id, T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        store
            .say(&task.id, "再说一句", T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        store
            .update_requirement(&task.id, "新要求", None, T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        store
            .update_write_scope(&task.id, "notes", T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    let report = NewReport {
        state: ReportState::Ready,
        origin: ReportOrigin::Agent,
        summary: "做完了".to_owned(),
        question: None,
    };
    let _ = report;
    assert_eq!(
        store
            .accept(&task.id, &ReportId::from("missing"), T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
}

#[test]
fn late_report_is_kept_in_history_but_does_not_change_cancelled_stage() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "取消后迟到报告".to_owned(),
                question: None,
            },
            T0 + 20,
        )
        .unwrap();
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.reports.len(), 1);
    assert_eq!(stored.latest_report().unwrap().summary, "取消后迟到报告");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
}

#[test]
fn group_accept_refuses_a_cancelled_task_without_accepting_others_silently() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    let report = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "先交结果".to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
    store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    let batch = store
        .accept_many(&[(task.id.clone(), report)], T0 + 20)
        .unwrap();
    assert_eq!(batch.outcomes.len(), 1);
    assert_eq!(
        batch.outcomes[0].refused.as_deref(),
        Some(CANCELLED_MESSAGE)
    );
}
