//! T084.2：用量等待只读事实和席位满时的泵睡眠。

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
    assert!(
        runtime
            .store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .reports
            .is_empty()
    );
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
    assert!(stored.reports.is_empty(), "用量等待不写报告，也不叫醒 Kind");
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
