//! 用量等待事务（Attempt Deferred + waiting 报告 + UsageResume，不排 relay）。

use std::path::PathBuf;

use super::*;

const T0: i64 = 1_700_000_000_000;

fn draft() -> NewTask {
    NewTask {
        quote: None,
        quote_source: None,
        kind_note: None,
        project_id: None,
        cwd_source: None,
        title: "长任务".to_owned(),
        requirement: "慢慢做".to_owned(),
        dod: "做完".to_owned(),
        cwd: PathBuf::from("/tmp/usage-wait-cwd"),
        conversation: ConversationId::new("kind-session"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-chat".to_owned(),
            effort_wanted: "default".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

#[test]
fn usage_wait_ends_the_attempt_and_enqueues_one_usage_resume_effect() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let task = store.create_task(draft(), T0).unwrap();
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    let resume_at = T0 + 20_000;

    store
        .begin_usage_wait(&task.id, attempt, resume_at, "deepseek", T0 + 2)
        .unwrap();

    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().and_then(|attempt| attempt.end),
        Some(AttemptEnd::Deferred)
    );
    assert_eq!(stored.reports.len(), 1, "保留一条 waiting 报告");
    assert_eq!(stored.reports[0].state, ReportState::Waiting);

    let key = format!("usage-resume:{}:{}", task.id, attempt.0);
    assert_eq!(
        store.pending_effect(EffectKind::UsageResume, &key).unwrap(),
        Some(EffectPayload::UsageResume {
            task_id: task.id.clone(),
            attempt_id: attempt,
            resume_at,
            connection: "deepseek".to_owned(),
        })
    );
    assert_eq!(
        store.next_effect_due_of(EffectKind::UsageResume).unwrap(),
        Some(resume_at)
    );
    assert_eq!(
        store.next_effect_due_of(EffectKind::Relay).unwrap(),
        None,
        "用量等待不排 relay，不会叫醒 Kind"
    );

    // 重复写同一等待：不新增报告，效果仍只有一条。
    store
        .begin_usage_wait(&task.id, attempt, resume_at, "deepseek", T0 + 3)
        .unwrap();
    assert_eq!(
        store
            .claim_effect(EffectKind::UsageResume, resume_at - 1)
            .unwrap()
            .is_none(),
        true
    );
    assert_eq!(
        store
            .claim_effect(EffectKind::UsageResume, resume_at)
            .unwrap()
            .map(|claim| claim.payload),
        Some(EffectPayload::UsageResume {
            task_id: task.id.clone(),
            attempt_id: attempt,
            resume_at,
            connection: "deepseek".to_owned(),
        })
    );
}

#[test]
fn terminal_report_and_non_latest_attempt_are_refused() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let task = store.create_task(draft(), T0).unwrap();
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Failed,
                origin: ReportOrigin::Agent,
                summary: "失败了".to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
    store
        .end_attempt(&task.id, attempt, T0 + 3, AttemptEnd::Reported)
        .unwrap();
    assert!(
        store
            .begin_usage_wait(&task.id, attempt, T0 + 20_000, "deepseek", T0 + 4)
            .is_err(),
        "已有终态报告的 Attempt 不能转成用量等待"
    );

    let second = store.begin_attempt(&task.id, T0 + 4).unwrap();
    let _ = second;
    let stale = AttemptId(attempt.0);
    assert!(
        store
            .begin_usage_wait(&task.id, stale, T0 + 20_000, "deepseek", T0 + 5)
            .is_err(),
        "非最新 Attempt 不能转成用量等待"
    );
}

#[test]
fn stop_usage_wait_appends_a_marker_and_removes_only_the_effect() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let task = store.create_task(draft(), T0).unwrap();
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .begin_usage_wait(&task.id, attempt, T0 + 20_000, "deepseek", T0 + 2)
        .unwrap();
    assert_eq!(store.task(&task.id).unwrap().unwrap().reports.len(), 1);

    assert!(store.stop_usage_wait(&task.id, attempt, T0 + 3).unwrap());
    let stored = store.task(&task.id).unwrap().unwrap();
    let latest = stored.latest_attempt().unwrap();
    assert_eq!(latest.end, Some(AttemptEnd::Deferred), "不改写 Deferred");
    assert_eq!(latest.ended_at, Some(T0 + 2), "不改写结束时间");
    assert_eq!(latest.stops.len(), 1, "只追加 stops");
    assert_eq!(stored.reports.len(), 1, "报告不改");
    assert!(stored.is_stopped());
    assert_eq!(store.stage(&task.id).unwrap(), Stage::NeedsConditions);
    let key = format!("usage-resume:{}:{}", task.id, attempt.0);
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store
            .begin_followup_with_handoffs(
                &task.id,
                AttemptTrigger::UsageResume,
                Vec::new(),
                T0 + 4,
            )
            .unwrap(),
        None,
        "停止标记后到点自动续跑不能再开 Attempt"
    );
}

#[test]
fn clear_usage_wait_is_for_early_continue_and_changes_no_task_facts() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let task = store.create_task(draft(), T0).unwrap();
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .begin_usage_wait(&task.id, attempt, T0 + 20_000, "deepseek", T0 + 2)
        .unwrap();
    let before = store.task(&task.id).unwrap().unwrap();
    assert!(store.clear_usage_wait(&task.id).unwrap());
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(before.reports.len(), after.reports.len());
    assert_eq!(
        after.latest_attempt().unwrap().end,
        Some(AttemptEnd::Deferred)
    );
    assert!(!after.is_stopped());
    let key = format!("usage-resume:{}:{}", task.id, attempt.0);
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cancel_removes_the_usage_resume_effect_without_rewriting_the_attempt() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let task = store.create_task(draft(), T0).unwrap();
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .begin_usage_wait(&task.id, attempt, T0 + 20_000, "deepseek", T0 + 2)
        .unwrap();
    store
        .cancel(&task.id, CancellationSource::User, None, T0 + 3)
        .unwrap();
    let key = format!("usage-resume:{}:{}", task.id, attempt.0);
    assert!(
        store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none()
    );
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().unwrap().end,
        Some(AttemptEnd::Deferred)
    );
    assert!(stored.cancelled.is_some());
}
