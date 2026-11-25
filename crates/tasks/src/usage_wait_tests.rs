//! 用量等待事务（Attempt Deferred + UsageResume，不写报告/不排 relay）。

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
    assert!(stored.reports.is_empty(), "用量等待不写报告");

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
        "用量等待不写报告、不排 relay"
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
