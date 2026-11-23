use std::sync::{Arc, Mutex, Weak};

use model::{DEFAULT_CONTEXT_WINDOW, Effort};
use tasks::{
    AttemptTrigger, ConversationId, GateCandidate, GateCheckOutcome, GateCommand, GateConclusion,
    MilestoneGate, ModelChoice, NewGraph, NewReport, NewSubtask, NewTask, ReportOrigin,
    ReportState, TaskPolicy, VerdictSource,
};

use super::*;
use crate::root::RootSessionOptions;

fn open_runtime(data_dir: &std::path::Path) -> Arc<TaskRuntime> {
    let options = RootSessionOptions {
        projects: None,
        data_dir: data_dir.to_path_buf(),
        cwd: data_dir.to_path_buf(),
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
    Arc::new(TaskRuntime::new(&options, Arc::new(Mutex::new(None)), Weak::new(), None).unwrap())
}

fn draft(title: &str) -> NewTask {
    NewTask {
        project_id: None,
        cwd_source: None,
        title: title.to_owned(),
        requirement: "把事做完".to_owned(),
        dod: "有结果".to_owned(),
        cwd: std::path::PathBuf::from("/tmp"),
        conversation: ConversationId::new("kind-1"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: THIN_TOOLS.iter().map(|name| (*name).to_owned()).collect(),
    }
}

struct GateFixture {
    _directory: tempfile::TempDir,
    runtime: Arc<TaskRuntime>,
    gate_id: TaskId,
    attempt_id: AttemptId,
}

struct LaunchFixture {
    _directory: tempfile::TempDir,
    runtime: Arc<TaskRuntime>,
    gate_id: TaskId,
}

async fn gate_launch_fixture(commands: Vec<GateCommand>) -> LaunchFixture {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join(TASKS_DB_FILE)).unwrap();
    let graph = NewGraph {
        root: draft("总任务"),
        subtasks: vec![
            NewSubtask {
                program_gate: Vec::new(),
                task: draft("被把关"),
                policy: TaskPolicy::default(),
                after: Vec::new(),
                milestone: false,
            },
            NewSubtask {
                program_gate: commands,
                task: draft("程序门"),
                policy: TaskPolicy::default(),
                after: vec![0],
                milestone: true,
            },
        ],
    };
    let root = store.create_graph(graph, 1_700_000_000_000).unwrap();
    store.confirm_graph(&root.id, 1_700_000_000_001).unwrap();
    let children = store.children(&root.id).unwrap();
    let guarded = children[0].clone();
    let gate = children[1].clone();
    let guarded_attempt = store.begin_attempt(&guarded.id, 1_700_000_000_002).unwrap();
    store
        .record_report(
            &guarded.id,
            guarded_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "改好了".to_owned(),
                question: None,
            },
            1_700_000_000_003,
        )
        .unwrap();
    let runtime = open_runtime(directory.path());
    LaunchFixture {
        _directory: directory,
        runtime,
        gate_id: gate.id,
    }
}

async fn gate_fixture(commands: Vec<GateCommand>) -> GateFixture {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join(TASKS_DB_FILE)).unwrap();
    let graph = NewGraph {
        root: draft("总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft("被把关"),
                policy: TaskPolicy::default(),
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
            NewSubtask {
                task: draft("程序门"),
                policy: TaskPolicy::default(),
                after: vec![0],
                milestone: true,
                program_gate: commands,
            },
        ],
    };
    let root = store.create_graph(graph, 1_700_000_000_000).unwrap();
    store.confirm_graph(&root.id, 1_700_000_000_001).unwrap();
    let children = store.children(&root.id).unwrap();
    let guarded = children[0].clone();
    let gate = children[1].clone();
    assert!(gate.is_program_gate());
    assert!(!gate.gate_commands().is_empty());
    assert_eq!(gate.commands_version(), 1);
    let guarded_attempt = store.begin_attempt(&guarded.id, 1_700_000_000_002).unwrap();
    store
        .record_report(
            &guarded.id,
            guarded_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "改好了".to_owned(),
                question: None,
            },
            1_700_000_000_003,
        )
        .unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    let handoff = gate.needs[0].handoff.clone().expect("候选已交接");
    let candidate = GateCandidate {
        report_id: handoff.report_id.clone(),
        requirement_version: handoff.requirement_version,
    };
    let attempt_id = store
        .begin_attempt_with(
            &gate.id,
            1_700_000_000_004,
            AttemptTrigger::GateRecheck { candidate },
            vec![handoff],
        )
        .unwrap();
    let runtime = open_runtime(directory.path());
    GateFixture {
        _directory: directory,
        runtime,
        gate_id: gate.id,
        attempt_id,
    }
}

async fn run(fixture: &GateFixture) -> (Task, Vec<GateCheck>) {
    let store = fixture.runtime.store();
    let root = fixture
        .runtime
        .data_dir
        .join("gate-checks")
        .join(format!("{}-{}", fixture.gate_id, fixture.attempt_id.0));
    let registry = Arc::new(TaskRegistry::detached(root));
    let running = Arc::new(RunningGate {
        attempt_id: fixture.attempt_id,
        registry: Arc::clone(&registry),
        job_id: Mutex::new(None),
        current_index: Mutex::new(None),
        stop_reason: Mutex::new(None),
        cancelled: AtomicBool::new(false),
    });
    fixture
        .runtime
        .clone()
        .run_program_gate(
            fixture.gate_id.clone(),
            fixture.attempt_id,
            registry,
            running,
        )
        .await;
    let gate = store.task(&fixture.gate_id).unwrap().unwrap();
    let report = gate.latest_report().expect("程序门报告");
    assert_eq!(report.origin, ReportOrigin::Program);
    (gate.clone(), report.checks.clone())
}

#[tokio::test]
async fn all_green_is_program_pass_with_checks_in_the_report() {
    let fixture = gate_fixture(vec![
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
        GateCommand {
            command: "printf ok".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let (gate, checks) = run(&fixture).await;
    assert_eq!(checks.len(), 2);
    assert!(
        checks
            .iter()
            .all(|check| check.outcome == GateCheckOutcome::Ran)
    );
    assert!(checks.iter().all(|check| check.exit_code == Some(0)));
    let verdict = gate.latest_verdict().expect("判定");
    assert_eq!(verdict.conclusion, GateConclusion::Pass);
    assert_eq!(verdict.judged_by, VerdictSource::Program);
    assert_eq!(verdict.commands_version, 1);
    let candidate = gate.latest_attempt().unwrap().handoffs[0].report_id.clone();
    assert!(gate.passed(&candidate));
}

#[tokio::test]
async fn one_nonzero_keeps_running_and_program_needs_fix() {
    let fixture = gate_fixture(vec![
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
        GateCommand {
            command: "exit 3".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let (gate, checks) = run(&fixture).await;
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[1].exit_code, Some(3));
    let verdict = gate.latest_verdict().expect("判定");
    assert_eq!(verdict.conclusion, GateConclusion::NeedsFix);
    let candidate = gate.latest_attempt().unwrap().handoffs[0].report_id.clone();
    assert!(!gate.passed(&candidate));
    assert!(gate.needs_fix(&candidate).is_some());
}

#[tokio::test]
async fn timeout_is_undetermined_and_later_commands_are_skipped() {
    let fixture = gate_fixture(vec![
        GateCommand {
            command: "sleep 5".to_owned(),
            timeout_ms: Some(200),
        },
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let (gate, checks) = run(&fixture).await;
    assert_eq!(checks.len(), 2);
    assert_eq!(checks[0].outcome, GateCheckOutcome::TimedOut);
    assert_eq!(checks[1].outcome, GateCheckOutcome::Skipped);
    let verdict = gate.latest_verdict().expect("判定");
    assert_eq!(verdict.conclusion, GateConclusion::Undetermined);
    let candidate = gate.latest_attempt().unwrap().handoffs[0].report_id.clone();
    assert!(!gate.passed(&candidate));
    assert!(gate.needs_fix(&candidate).is_none());
}

#[tokio::test]
async fn command_env_has_no_aios_or_secret_names() {
    let fixture = gate_fixture(vec![
        GateCommand {
            command: "env".to_owned(),
            timeout_ms: None,
        },
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let (_gate, checks) = run(&fixture).await;
    let tail = &checks[0].tail;
    assert!(!tail.contains("AIOS_"), "{tail}");
    assert!(!tail.contains("TOKEN="), "{tail}");
    assert!(!tail.contains("KEY="), "{tail}");
}

#[test]
fn program_gate_serializes_commands_and_version() {
    let gate = MilestoneGate {
        commands: vec![GateCommand {
            command: "cargo test".to_owned(),
            timeout_ms: Some(5_000),
        }],
        commands_version: 2,
    };
    let json = serde_json::to_string(&gate).unwrap();
    assert!(json.contains("commands_version"));
    let old: MilestoneGate = serde_json::from_str("{}").unwrap();
    assert!(!old.is_program_gate());
    assert_eq!(old.commands_version, 0);
}

#[tokio::test]
async fn program_gate_launch_effect_spawns_runner_without_a_session() {
    let fixture = gate_launch_fixture(vec![
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
        GateCommand {
            command: "test -d .".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let store = fixture.runtime.store();
    store
        .enqueue_effect(
            EffectPayload::Launch {
                task_id: fixture.gate_id.clone(),
            },
            1_700_000_000_010,
        )
        .unwrap();
    let mut launched = 0;
    while let Ok(Some(claim)) = store.claim_effect(EffectKind::Launch, 1_700_000_000_011) {
        let gate_id = match &claim.payload {
            EffectPayload::Launch { task_id } => task_id.clone(),
            _ => unreachable!(),
        };
        if gate_id == fixture.gate_id {
            let _ = fixture.runtime.launch(claim).await;
            launched += 1;
            break;
        }
        store.complete_effect(&claim).unwrap();
    }
    assert_eq!(launched, 1);
    let mut report = None;
    for _ in 0..100 {
        let gate = store.task(&fixture.gate_id).unwrap().unwrap();
        if let Some(latest) = gate
            .reports
            .iter()
            .find(|report| report.origin == ReportOrigin::Program)
        {
            report = Some(latest.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let report = report.expect("launch 入口应跑出程序门报告");
    assert_eq!(report.checks.len(), 2);
    assert_eq!(
        store
            .task(&fixture.gate_id)
            .unwrap()
            .unwrap()
            .latest_verdict()
            .unwrap()
            .conclusion,
        GateConclusion::Pass
    );
}

#[tokio::test]
async fn update_commands_bumps_version_and_old_verdict_does_not_match() {
    let fixture = gate_fixture(vec![GateCommand {
        command: "true".to_owned(),
        timeout_ms: None,
    }])
    .await;
    let (gate, _) = run(&fixture).await;
    let candidate = gate.latest_attempt().unwrap().handoffs[0].report_id.clone();
    assert!(gate.passed(&candidate));
    let store = fixture.runtime.store();
    let updated = store
        .update_gate_commands(
            &fixture.gate_id,
            vec![GateCommand {
                command: "false".to_owned(),
                timeout_ms: None,
            }],
            1_700_000_000_020,
        )
        .unwrap();
    assert_eq!(updated.commands_version(), 2);
    assert_eq!(updated.gate_commands()[0].command, "false");
    // 旧判定绑的是第 1 版，不再算这道门的通过。
    assert!(!updated.passed(&candidate));
    assert!(updated.needs_fix(&candidate).is_none());
    // 跑着的时候不许改。
    let gate_now = store.task(&fixture.gate_id).unwrap().unwrap();
    let handoff = gate_now.needs[0].handoff.clone().unwrap();
    store
        .begin_attempt_with(
            &fixture.gate_id,
            1_700_000_000_021,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: handoff.report_id.clone(),
                    requirement_version: handoff.requirement_version,
                },
            },
            vec![handoff],
        )
        .unwrap();
    assert!(matches!(
        store.update_gate_commands(
            &fixture.gate_id,
            vec![GateCommand {
                command: "true".to_owned(),
                timeout_ms: None,
            }],
            1_700_000_000_022,
        ),
        Err(TaskError::GateWhileRunning)
    ));
}

#[tokio::test]
async fn stopping_a_program_gate_is_undetermined_not_pass() {
    let fixture = gate_launch_fixture(vec![GateCommand {
        command: "sleep 5".to_owned(),
        timeout_ms: None,
    }])
    .await;
    let store = fixture.runtime.store();
    // 跳过被把关任务的 launch 效果，手动替门排一次 launch 并走 runtime.launch 入口。
    let mut launched = false;
    loop {
        let claim = store
            .claim_effect(EffectKind::Launch, 1_700_000_000_010)
            .unwrap();
        let Some(claim) = claim else { break };
        if matches!(&claim.payload, EffectPayload::Launch { task_id } if *task_id == fixture.gate_id)
        {
            let _ = fixture.runtime.launch(claim).await;
            launched = true;
            break;
        }
        store.complete_effect(&claim).unwrap();
    }
    assert!(launched);
    for _ in 0..100 {
        if store
            .task(&fixture.gate_id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let response = fixture
        .runtime
        .stop_task(fixture.gate_id.as_str(), StopMode::Now)
        .await
        .unwrap();
    assert!(response.stopped);
    let mut gate = None;
    for _ in 0..200 {
        let loaded = store.task(&fixture.gate_id).unwrap().unwrap();
        if loaded
            .reports
            .iter()
            .any(|report| report.origin == ReportOrigin::Program)
        {
            gate = Some(loaded);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let gate = gate.expect("停止后应有未确定报告");
    let report = gate.latest_report().unwrap();
    assert_eq!(report.checks[0].outcome, GateCheckOutcome::Stopped);
    assert_eq!(
        gate.latest_verdict().unwrap().conclusion,
        GateConclusion::Undetermined
    );
    assert_eq!(
        store.stage(&fixture.gate_id).unwrap(),
        Stage::NeedsConditions
    );
    assert!(!gate.passed(&gate.latest_attempt().unwrap().handoffs[0].report_id));
}

#[tokio::test]
async fn boot_scan_marks_an_open_program_gate_interrupted_with_restart() {
    let fixture = gate_launch_fixture(vec![GateCommand {
        command: "true".to_owned(),
        timeout_ms: None,
    }])
    .await;
    let store = fixture.runtime.store();
    let gate = store.task(&fixture.gate_id).unwrap().unwrap();
    let handoff = gate.needs[0].handoff.clone().unwrap();
    store
        .begin_attempt_with(
            &fixture.gate_id,
            1_700_000_000_010,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: handoff.report_id.clone(),
                    requirement_version: handoff.requirement_version,
                },
            },
            vec![handoff],
        )
        .unwrap();
    let boot = BootId::from("boot-t057");
    let scan = store.scan_at_boot(&boot, 1_700_000_000_011).unwrap();
    assert!(scan.interrupted.contains(&fixture.gate_id));
    let gate = store.task(&fixture.gate_id).unwrap().unwrap();
    assert_eq!(
        gate.latest_attempt().unwrap().end,
        Some(AttemptEnd::Interrupted)
    );
    assert!(
        store
            .effect_pending(EffectKind::Restart, boot.as_str())
            .unwrap()
    );
}

#[tokio::test]
async fn gate_check_again_queues_launch_only_after_undetermined() {
    let fixture = gate_fixture(vec![GateCommand {
        command: "sleep 5".to_owned(),
        timeout_ms: Some(100),
    }])
    .await;
    let _ = run(&fixture).await;
    let store = fixture.runtime.store();
    fixture
        .runtime
        .gate_check_again(&fixture.gate_id)
        .expect("未确定后可以重跑");
    assert!(
        store
            .effect_pending(EffectKind::Launch, fixture.gate_id.as_str())
            .unwrap()
    );
}

#[tokio::test]
async fn gate_check_again_refuses_four_other_states() {
    let fixture = gate_fixture(vec![
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
        GateCommand {
            command: "printf ok".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let _ = run(&fixture).await;
    let store = fixture.runtime.store();
    let error = fixture
        .runtime
        .gate_check_again(&fixture.gate_id)
        .expect_err("通过后不再重跑");
    assert!(error.contains("不是未确定/被打断"), "{error}");

    // 候选变了：被把关任务又交一份 ready 结果。
    let gate = store.task(&fixture.gate_id).unwrap().unwrap();
    let guarded = store.task(&gate.needs[0].from).unwrap().unwrap();
    if let Some(open) = guarded
        .latest_attempt()
        .filter(|attempt| attempt.ended_at.is_none())
    {
        store
            .end_attempt(
                &guarded.id,
                open.id,
                1_700_000_000_030,
                AttemptEnd::Reported,
            )
            .ok();
    }
    let attempt = store.begin_attempt(&guarded.id, 1_700_000_000_031).unwrap();
    store
        .record_report(
            &guarded.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "又改了一版".to_owned(),
                question: None,
            },
            1_700_000_000_032,
        )
        .unwrap();
    let error = fixture
        .runtime
        .gate_check_again(&fixture.gate_id)
        .expect_err("候选变了不重跑");
    assert!(error.contains("候选"), "{error}");

    // 普通任务不是门。
    let task = store
        .create_task(
            NewTask {
                project_id: None,
                cwd_source: None,
                title: "普通任务".to_owned(),
                requirement: "做".to_owned(),
                dod: "好".to_owned(),
                cwd: std::path::PathBuf::from("/tmp"),
                conversation: ConversationId::new("kind-1"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: THIN_TOOLS.iter().map(|name| (*name).to_owned()).collect(),
            },
            1_700_000_000_033,
        )
        .unwrap();
    let error = fixture
        .runtime
        .gate_check_again(&task.id)
        .expect_err("普通任务不是门");
    assert!(error.contains("不是门"), "{error}");
}

#[tokio::test]
async fn interrupted_program_gate_can_be_rerun_but_not_resumed() {
    let fixture = gate_launch_fixture(vec![GateCommand {
        command: "true".to_owned(),
        timeout_ms: None,
    }])
    .await;
    let store = fixture.runtime.store();
    let gate = store.task(&fixture.gate_id).unwrap().unwrap();
    let handoff = gate.needs[0].handoff.clone().unwrap();
    store
        .begin_attempt_with(
            &fixture.gate_id,
            1_700_000_000_040,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: handoff.report_id.clone(),
                    requirement_version: handoff.requirement_version,
                },
            },
            vec![handoff],
        )
        .unwrap();
    let boot = BootId::from("boot-t057-resume");
    store.scan_at_boot(&boot, 1_700_000_000_041).unwrap();
    let error = fixture
        .runtime
        .resume_task(&fixture.gate_id)
        .await
        .expect_err("程序门没有会话可接");
    assert!(error.contains("gate_check_again"), "{error}");
    fixture
        .runtime
        .gate_check_again(&fixture.gate_id)
        .expect("被打断后可以重跑");
    assert!(
        store
            .effect_pending(EffectKind::Launch, fixture.gate_id.as_str())
            .unwrap()
    );
}

#[tokio::test]
async fn gate_check_again_refuses_changed_commands_version() {
    let fixture = gate_fixture(vec![GateCommand {
        command: "true".to_owned(),
        timeout_ms: None,
    }])
    .await;
    let _ = run(&fixture).await;
    fixture
        .runtime
        .store()
        .update_gate_commands(
            &fixture.gate_id,
            vec![GateCommand {
                command: "false".to_owned(),
                timeout_ms: None,
            }],
            1_700_000_000_030,
        )
        .unwrap();
    let error = fixture
        .runtime
        .gate_check_again(&fixture.gate_id)
        .expect_err("命令版本变了不重跑");
    assert!(error.contains("命令已经改过"), "{error}");
}

#[tokio::test]
async fn candidate_changes_while_gate_runs_then_new_candidate_gets_verdict() {
    let fixture = gate_launch_fixture(vec![
        GateCommand {
            command: "sleep 3".to_owned(),
            timeout_ms: None,
        },
        GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        },
    ])
    .await;
    let store = fixture.runtime.store();
    let id = fixture.gate_id.as_str();
    let mut launched = false;
    while let Some(claim) = store
        .claim_effect(EffectKind::Launch, 1_700_000_000_010)
        .unwrap()
    {
        let task_id = match &claim.payload {
            EffectPayload::Launch { task_id } => task_id.clone(),
            _ => unreachable!(),
        };
        if task_id == fixture.gate_id {
            let _ = fixture.runtime.launch(claim).await;
            launched = true;
            break;
        }
        store.complete_effect(&claim).unwrap();
    }
    assert!(launched, "门 launch 效果应能领到");
    let mut running = false;
    for _ in 0..300 {
        let gate = store.task(&fixture.gate_id).unwrap().unwrap();
        if gate
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_none())
        {
            running = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(running, "门应先在跑 sleep 3");
    let gate = store.task(&fixture.gate_id).unwrap().unwrap();
    let guarded_id = gate.needs[0].from.clone();
    let guarded = store.task(&guarded_id).unwrap().unwrap();
    if let Some(open) = guarded
        .latest_attempt()
        .filter(|attempt| attempt.ended_at.is_none())
    {
        store
            .end_attempt(
                &guarded_id,
                open.id,
                1_700_000_000_020,
                AttemptEnd::Reported,
            )
            .unwrap();
    }
    let attempt = store.begin_attempt(&guarded_id, 1_700_000_000_021).unwrap();
    let new_report = store
        .record_report(
            &guarded_id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第二份".to_owned(),
                question: None,
            },
            1_700_000_000_022,
        )
        .unwrap();
    let _ = id;
    // 第一次门跑完后，F1 会排一次 launch；测试里没有常驻效果泵，手动领并开第二轮。
    for _ in 0..200 {
        let gate = store.task(&fixture.gate_id).unwrap().unwrap();
        if gate
            .latest_attempt()
            .is_some_and(|attempt| attempt.ended_at.is_some())
        {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    let mut second = false;
    for _ in 0..300 {
        if let Some(claim) = store
            .claim_effect(EffectKind::Launch, i64::MAX / 2)
            .unwrap()
        {
            let task_id = match &claim.payload {
                EffectPayload::Launch { task_id } => task_id.clone(),
                _ => unreachable!(),
            };
            if task_id == fixture.gate_id {
                let _ = fixture.runtime.launch(claim).await;
                second = true;
                break;
            }
            store.complete_effect(&claim).unwrap();
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(second, "F1 应对最新候选排第二次 launch");
    let mut seen = false;
    for _ in 0..500 {
        let gate = store.task(&fixture.gate_id).unwrap().unwrap();
        let reports = gate
            .reports
            .iter()
            .filter(|report| report.origin == ReportOrigin::Program)
            .count();
        if reports >= 2
            && gate
                .latest_verdict()
                .is_some_and(|verdict| verdict.candidate.report_id == new_report)
        {
            seen = true;
            assert_eq!(
                gate.latest_verdict().unwrap().conclusion,
                GateConclusion::Pass
            );
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    if !seen {
        let gate = store.task(&fixture.gate_id).unwrap().unwrap();
        eprintln!(
            "F1 DEBUG attempts={:?} reports={} verdicts={:?} handoff={:?} launch_pending={}",
            gate.latest_attempt()
                .map(|attempt| (attempt.id, attempt.ended_at, attempt.end)),
            gate.reports.len(),
            gate.verdicts
                .iter()
                .map(|verdict| (verdict.candidate.report_id.clone(), verdict.conclusion))
                .collect::<Vec<_>>(),
            gate.needs[0]
                .handoff
                .as_ref()
                .map(|handoff| handoff.report_id.clone()),
            store
                .effect_pending(EffectKind::Launch, gate.id.as_str())
                .unwrap()
        );
    }
    assert!(seen, "门跑完后应自动对最新候选再跑并出判定");
}
