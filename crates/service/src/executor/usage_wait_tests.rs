//! 用量等待只读事实和席位满时的泵睡眠。

use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};

use super::*;
use crate::root::RootSessionOptions;
use model::{DEFAULT_CONTEXT_WINDOW, Effort};

const T0: i64 = 1_800_000_000_000;

fn runtime() -> (tempfile::TempDir, Arc<TaskRuntime>) {
    let directory = tempfile::tempdir().unwrap();
    let options = RootSessionOptions {
        projects: None,
        data_dir: directory.path().to_path_buf(),
        cwd: directory.path().to_path_buf(),
        model: model::DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: DEFAULT_CONTEXT_WINDOW,
            effort: Effort::new("high"),
            tool_update: None,
        },
        config: agent::AgentConfig::default(),
        credentials: None,
        identity: None,
        model_factory: None,
        append_hook_factory: None,
        keyring_changed: None,
        actions: Arc::new(crate::actions::ActionRegistry::empty()),
        exa_base_url: None,
        web_listen: None,
        task_runtime: None,
    };
    let runtime =
        TaskRuntime::new(&options, Arc::new(Mutex::new(None)), Weak::new(), None).unwrap();
    (directory, Arc::new(runtime))
}

fn draft(cwd: PathBuf) -> NewTask {
    NewTask {
        quote: None,
        quote_source: None,
        kind_note: None,
        project_id: None,
        cwd_source: None,
        title: "用量等待".to_owned(),
        requirement: "等恢复".to_owned(),
        dod: "接着做".to_owned(),
        cwd,
        conversation: ConversationId::new("kind-usage-wait"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: Vec::new(),
    }
}

#[test]
fn wait_fact_lives_while_the_resume_effect_is_pending() {
    let (directory, runtime) = runtime();
    let task = runtime
        .store
        .create_task(draft(directory.path().to_path_buf()), T0)
        .unwrap();
    let launch = runtime
        .store
        .claim_effect(EffectKind::Launch, T0)
        .unwrap()
        .unwrap();
    runtime.store.complete_effect(&launch).unwrap();
    let attempt = runtime.store.begin_attempt(&task.id, T0 + 1).unwrap();
    let resume_at = T0 + 40_000;
    runtime
        .store
        .begin_usage_wait(&task.id, attempt, resume_at, "deepseek", T0 + 2)
        .unwrap();
    let stored = runtime.store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        runtime.task_usage_wait(&stored),
        Some(crate::protocol::TaskReadUsageWait {
            resume_at,
            connection: "deepseek".to_owned(),
        })
    );
    let stored = runtime.store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.reports.len(), 1, "保留一条 waiting 报告");
    assert_eq!(stored.reports[0].state, ReportState::Waiting);
}

#[test]
fn full_seats_keep_the_usage_due_out_of_the_sleep_time() {
    let (directory, runtime) = runtime();
    let task = runtime
        .store
        .create_task(draft(directory.path().to_path_buf()), T0)
        .unwrap();
    let launch = runtime
        .store
        .claim_effect(EffectKind::Launch, T0)
        .unwrap()
        .unwrap();
    runtime.store.complete_effect(&launch).unwrap();
    let attempt = runtime.store.begin_attempt(&task.id, T0 + 1).unwrap();
    let resume_at = T0 + 40_000;
    runtime
        .store
        .begin_usage_wait(&task.id, attempt, resume_at, "deepseek", T0 + 2)
        .unwrap();

    runtime.set_exec_seats(1);
    assert_eq!(runtime.next_wake_at(), Some(resume_at));
    runtime.set_exec_seats(0);
    assert_eq!(
        runtime.next_wake_at(),
        None,
        "席位满时不能把 UsageResume 的 due 当可醒时间，否则会空转"
    );
    runtime.set_exec_seats(1);
    assert_eq!(runtime.next_wake_at(), Some(resume_at));
    runtime.set_exec_paused(true);
    assert_eq!(runtime.next_wake_at(), None, "全局暂停时不醒");
    runtime.set_exec_paused(false);
    assert_eq!(runtime.next_wake_at(), Some(resume_at));
}

#[test]
fn turn_ended_with_long_429_creates_the_wait_without_relay_or_host() {
    let (directory, runtime) = runtime();
    let task = runtime
        .store
        .create_task(draft(directory.path().to_path_buf()), T0)
        .unwrap();
    let launch = runtime
        .store
        .claim_effect(EffectKind::Launch, T0)
        .unwrap()
        .unwrap();
    runtime.store.complete_effect(&launch).unwrap();
    let attempt = runtime.store.begin_attempt(&task.id, T0 + 1).unwrap();
    let resume_at = T0 + 40_000;
    let conversation = session::SessionId::generate();
    let outcome = agent::TurnOutcome::Error {
        code: "RATE_LIMIT".to_owned(),
        message: "限流".to_owned(),
        resume_at_ms: Some(resume_at),
    };
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        runtime
            .turn_ended(&task.id, attempt, &conversation, outcome)
            .await;
    });
    let stored = runtime.store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().and_then(|attempt| attempt.end),
        Some(AttemptEnd::Deferred)
    );
    assert_eq!(stored.reports.len(), 1, "保留一条 waiting 报告");
    assert_eq!(stored.reports[0].state, ReportState::Waiting);
    assert_eq!(
        runtime.task_usage_wait(&stored),
        Some(crate::protocol::TaskReadUsageWait {
            resume_at,
            connection: "deepseek".to_owned(),
        })
    );
    assert_eq!(
        runtime.store.next_effect_due_of(EffectKind::Relay).unwrap(),
        None,
        "等待事实不能排 relay"
    );
}

#[tokio::test]
async fn due_resume_without_a_session_completes_the_effect_and_keeps_the_wait_facts() {
    let (directory, runtime) = runtime();
    let task = runtime
        .store
        .create_task(draft(directory.path().to_path_buf()), T0)
        .unwrap();
    let launch = runtime
        .store
        .claim_effect(EffectKind::Launch, T0)
        .unwrap()
        .unwrap();
    runtime.store.complete_effect(&launch).unwrap();
    let attempt = runtime.store.begin_attempt(&task.id, T0 + 1).unwrap();
    let resume_at = T0 + 40_000;
    runtime
        .store
        .begin_usage_wait(&task.id, attempt, resume_at, "deepseek", T0 + 2)
        .unwrap();
    let claim = runtime
        .store
        .claim_effect(EffectKind::UsageResume, resume_at)
        .unwrap()
        .expect("到点效果");
    runtime.deliver_usage_resume(claim).await;
    let stored = runtime.store.task(&task.id).unwrap().unwrap();
    assert_eq!(
        stored.latest_attempt().and_then(|attempt| attempt.end),
        Some(AttemptEnd::Deferred),
        "接不上会话不改写 Deferred"
    );
    assert_eq!(stored.reports.len(), 1, "等待报告还在");
    let key = format!("usage-resume:{}:{}", task.id, attempt.0);
    assert!(
        runtime
            .store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none(),
        "NoSession 后效果收掉，不无限重试"
    );
}

#[tokio::test]
async fn due_resume_with_a_missing_package_ends_the_new_attempt_and_completes_the_effect() {
    let (directory, runtime) = runtime();
    let task = runtime
        .store
        .create_task(draft(directory.path().to_path_buf()), T0)
        .unwrap();
    let launch = runtime
        .store
        .claim_effect(EffectKind::Launch, T0)
        .unwrap()
        .unwrap();
    runtime.store.complete_effect(&launch).unwrap();
    let conversation = session::SessionId::generate();
    let attempt = runtime
        .store
        .begin_attempt_with(&task.id, T0 + 1, AttemptTrigger::Launch, Vec::new())
        .unwrap();
    runtime
        .store
        .attach_session(
            &task.id,
            attempt,
            ConversationId::new(conversation.as_str()),
        )
        .unwrap();
    let resume_at = T0 + 40_000;
    runtime
        .store
        .begin_usage_wait(&task.id, attempt, resume_at, "deepseek", T0 + 2)
        .unwrap();
    runtime
        .store
        .record_package_check(&task.id, vec![MissingItem::new("缺执行目录").unwrap()])
        .unwrap();
    let claim = runtime
        .store
        .claim_effect(EffectKind::UsageResume, resume_at)
        .unwrap()
        .expect("到点效果");
    runtime.deliver_usage_resume(claim).await;
    let stored = runtime.store.task(&task.id).unwrap().unwrap();
    let latest = stored.latest_attempt().unwrap();
    assert_eq!(
        latest.end,
        Some(AttemptEnd::NotOpened),
        "缺项的新 Attempt 要收尾"
    );
    assert!(!stored.missing.is_empty());
    let key = format!("usage-resume:{}:{}", task.id, attempt.0);
    assert!(
        runtime
            .store
            .pending_effect(EffectKind::UsageResume, &key)
            .unwrap()
            .is_none(),
        "PackageMissing 后效果收掉"
    );
}
