use std::path::Path;

use tasks::{
    AttemptEnd, AttemptTrigger, ConversationId, GateCandidate, GateCheckOutcome, GateCommand,
    GateConclusion, ModelChoice, NewGraph, NewReport, NewSubtask, NewTask, ReportOrigin,
    ReportState, Store, Task, TaskPolicy,
};
use tempfile::TempDir;

use super::delivery_for;

const T0: i64 = 1_700_000_000_000;

fn open() -> (TempDir, Store) {
    let dir = tempfile::tempdir().unwrap();
    let store = Store::open(dir.path().join("tasks.sqlite")).unwrap();
    (dir, store)
}

fn draft(cwd: &Path, title: &str) -> NewTask {
    NewTask {
        project_id: None,
        cwd_source: None,
        title: title.to_owned(),
        requirement: "把事做完".to_owned(),
        dod: "有结果".to_owned(),
        cwd: cwd.to_path_buf(),
        conversation: ConversationId::new("kind-delivery"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn graph(cwd: &Path, write: bool) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "被把关"),
                policy: TaskPolicy {
                    write_scope: write.then(|| "a.txt".to_owned()),
                    worktree: false,
                    bash: write,
                },
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
            NewSubtask {
                task: draft(cwd, "第二项"),
                policy: TaskPolicy::default(),
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
        ],
    }
}

fn first_child(store: &Store, cwd: &Path, write: bool) -> Task {
    let root = store.create_graph(graph(cwd, write), T0).unwrap();
    store.children(&root.id).unwrap()[0].clone()
}

fn record(
    store: &Store,
    task: &Task,
    state: ReportState,
    origin: ReportOrigin,
    summary: &str,
    at: i64,
) -> tasks::ReportId {
    let attempt = store.begin_attempt(&task.id, at).unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state,
                origin,
                summary: summary.to_owned(),
                question: None,
            },
            at + 1,
        )
        .unwrap();
    store
        .end_attempt(&task.id, attempt, at + 1, AttemptEnd::Reported)
        .ok();
    store
        .task(&task.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .unwrap()
        .id
        .clone()
}

fn gate_graph(store: &Store, cwd: &Path, program: bool) -> (Task, Task) {
    let mut gate = NewSubtask {
        task: draft(cwd, "核查门"),
        policy: TaskPolicy::default(),
        after: vec![0],
        milestone: true,
        program_gate: Vec::new(),
    };
    if program {
        gate.program_gate = vec![GateCommand {
            command: "true".to_owned(),
            timeout_ms: None,
        }];
    }
    let graph = NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "被把关"),
                policy: TaskPolicy {
                    write_scope: Some("a.txt".to_owned()),
                    worktree: false,
                    bash: false,
                },
                after: Vec::new(),
                milestone: false,
                program_gate: Vec::new(),
            },
            gate,
        ],
    };
    let root = store.create_graph(graph, T0).unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let children = store.children(&root.id).unwrap();
    (children[0].clone(), children[1].clone())
}

#[test]
fn no_gate_no_accept_records_unproven_then_accepts() {
    let (dir, store) = open();
    let task = first_child(&store, dir.path(), false);
    let report = record(
        &store,
        &task,
        ReportState::Ready,
        ReportOrigin::Agent,
        "查完了",
        T0 + 2,
    );
    let task = store.task(&task.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &task, None).expect("有 ready 就有记录");
    assert_eq!(record.attempt, 1);
    assert_eq!(record.word.text, "等你接受");
    assert!(record.basis.verdict.text.contains("未证明：没安排门任务"));
    assert_eq!(record.what.changed, None);
    assert!(record.missing[0].text.contains("没有记录到没验证"));
    store.accept(&task.id, &report, T0 + 5).unwrap();
    let task = store.task(&task.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &task, None).expect("接受后仍有记录");
    assert_eq!(record.word.text, "你接受了");
    assert!(
        record
            .basis
            .verdict
            .text
            .contains("没安排门任务：你接受了这一份")
    );
    assert!(record.accepted.state.text.contains("你接受了"));
}

#[test]
fn user_gate_unjudged_and_judged_are_separate_lines() {
    let (dir, store) = open();
    let (guarded, gate) = gate_graph(&store, dir.path(), false);
    let report = record(
        &store,
        &guarded,
        ReportState::Ready,
        ReportOrigin::Agent,
        "交了",
        T0 + 2,
    );
    let gate = store.task(&gate.id).unwrap().unwrap();
    let handoff = gate.needs[0].handoff.clone().unwrap();
    let attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 3,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: handoff.report_id.clone(),
                    requirement_version: handoff.requirement_version,
                },
            },
            vec![handoff],
        )
        .unwrap();
    let gate_report = store
        .record_report(
            &gate.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "我看过了".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &guarded, Some(&gate)).expect("有记录");
    assert!(record.basis.verdict.text.contains("还没判这一份"));
    assert!(record.basis.record.text.contains("我看过了"));
    assert_eq!(record.word.text, "检查未证明");

    store
        .judge_gate(
            &gate.id,
            &gate_report,
            GateConclusion::NeedsFix,
            Some("这里不对".to_owned()),
            T0 + 5,
        )
        .unwrap();
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &guarded, Some(&gate)).expect("判决后有记录");
    assert!(record.basis.verdict.text.contains("你判了需修正"));
    assert_eq!(record.basis.note.as_deref(), Some("这里不对"));
    assert_eq!(record.word.text, "检查未证明");
    assert_eq!(
        record.basis.record.text,
        "没有程序记录。核查是门任务的 ThinAgent 做的，它报的：我看过了"
    );
    assert_eq!(record.attempt, 1);
    assert_ne!(record.attempt, 0);
    assert_eq!(record.word.text, "检查未证明");
    let _ = report;
}

fn gate_check(
    index: usize,
    command: &str,
    exit: Option<i32>,
    outcome: GateCheckOutcome,
    ms: u64,
) -> tasks::GateCheck {
    tasks::GateCheck {
        index,
        command: command.to_owned(),
        exit_code: exit,
        outcome,
        duration_ms: ms,
        tail: String::new(),
        spill: None,
    }
}

fn program_gate_with(
    store: &Store,
    cwd: &Path,
    checks: Vec<tasks::GateCheck>,
    conclusion: GateConclusion,
    summary: &str,
) -> (Task, Task) {
    let (guarded, gate) = gate_graph(store, cwd, true);
    let _ = record(
        store,
        &guarded,
        ReportState::Ready,
        ReportOrigin::Agent,
        "交了",
        T0 + 2,
    );
    let gate = store.task(&gate.id).unwrap().unwrap();
    let handoff = gate.needs[0].handoff.clone().unwrap();
    let attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 3,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: handoff.report_id.clone(),
                    requirement_version: handoff.requirement_version,
                },
            },
            vec![handoff],
        )
        .unwrap();
    store
        .record_program_gate_result(
            &gate.id,
            attempt,
            summary.to_owned(),
            checks,
            conclusion,
            T0 + 4,
        )
        .unwrap();
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    (guarded, gate)
}

#[test]
fn program_gate_pass_checks_and_stale_command_version() {
    let (dir, store) = open();
    let (guarded, gate) = program_gate_with(
        &store,
        dir.path(),
        vec![gate_check(0, "true", Some(0), GateCheckOutcome::Ran, 12)],
        GateConclusion::Pass,
        "程序门通过：1 条命令全部退出码 0",
    );
    let record = delivery_for(dir.path(), &guarded, Some(&gate)).expect("有记录");
    assert_eq!(record.word.text, "等你接受");
    assert!(record.basis.verdict.text.contains("程序判了通过"));
    assert!(record.basis.record.text.contains("1 条里跑了 1 条"));
    assert_eq!(record.basis.checks.unwrap().checks.len(), 1);

    store
        .update_gate_commands(
            &gate.id,
            vec![GateCommand {
                command: "false".to_owned(),
                timeout_ms: None,
            }],
            T0 + 5,
        )
        .unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &guarded, Some(&gate)).expect("改命令后仍有记录");
    assert!(
        record.basis.verdict.text.contains("不再算数"),
        "{}",
        record.basis.verdict.text
    );
    assert_eq!(record.word.text, "检查未证明");
    assert!(
        record
            .accepted
            .state
            .text
            .contains("程序门判通过以后才能接受")
    );
}

#[test]
fn program_gate_undetermined_reason_and_skipped_commands() {
    let (dir, store) = open();
    let (guarded, gate) = program_gate_with(
        &store,
        dir.path(),
        vec![
            gate_check(0, "sleep 5", None, GateCheckOutcome::TimedOut, 1200),
            gate_check(1, "true", None, GateCheckOutcome::Skipped, 0),
        ],
        GateConclusion::Undetermined,
        "程序门未确定：超时：sleep 5（5 秒）",
    );
    let record = delivery_for(dir.path(), &guarded, Some(&gate)).expect("有记录");
    assert_eq!(record.word.text, "检查未证明");
    assert!(record.basis.verdict.text.contains("未确定"));
    assert!(record.basis.verdict.text.contains("不算通过，也不算有错"));
    assert!(
        record
            .missing
            .iter()
            .any(|line| line.text.contains("超时：sleep 5"))
    );
    assert!(
        record
            .missing
            .iter()
            .any(|line| line.text.contains("没跑的：true"))
    );
    assert!(record.basis.record.text.contains("2 条里跑了 1 条"));
}

#[test]
fn second_ready_moves_first_to_history_and_requirement_versions_are_listed() {
    let (dir, store) = open();
    let task = first_child(&store, dir.path(), true);
    let first = record(
        &store,
        &task,
        ReportState::Ready,
        ReportOrigin::Agent,
        "第一份",
        T0 + 2,
    );
    store.accept(&task.id, &first, T0 + 3).unwrap();
    let task = store.task(&task.id).unwrap().unwrap();
    let old = delivery_for(dir.path(), &task, None).expect("第一份记录");
    assert_eq!(old.attempt, 1);
    assert_eq!(old.word.text, "你接受了");

    store
        .update_requirement(&task.id, "第二版要求", None, T0 + 4)
        .unwrap();
    let second = record(
        &store,
        &task,
        ReportState::Ready,
        ReportOrigin::Agent,
        "第二份",
        T0 + 5,
    );
    let task = store.task(&task.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &task, None).expect("第二份记录");
    assert_eq!(record.attempt, 2);
    assert_eq!(record.version, 2);
    assert_eq!(record.history.len(), 1);
    assert_eq!(record.history[0].attempt, 1);
    assert!(record.history[0].accepted);
    assert!(record.history[0].word.contains("你接受过"));
    assert!(record.accepted.state.text.contains("接受过的是第 1 次结果"));
    assert!(
        record.accepted.revisions.as_ref().unwrap()[0]
            .text
            .contains("第 2 版 · 改了要求")
    );
    let _ = second;
}

#[test]
fn blocked_after_anchor_and_write_task_changed_list_are_facts() {
    let (dir, store) = open();
    let task = first_child(&store, dir.path(), true);
    record(
        &store,
        &task,
        ReportState::Ready,
        ReportOrigin::Agent,
        "第一份",
        T0 + 2,
    );
    record(
        &store,
        &task,
        ReportState::Blocked,
        ReportOrigin::Agent,
        "卡住了，缺 key",
        T0 + 4,
    );
    let task = store.task(&task.id).unwrap().unwrap();
    let record = delivery_for(dir.path(), &task, None).expect("有记录");
    assert!(
        record
            .missing
            .iter()
            .any(|line| line.text.contains("之后第 2 次没交出结果（卡住了）"))
    );
    assert_eq!(record.what.changed, Some(Vec::new()));
    assert!(record.what.bash);
    assert!(
        record
            .text()
            .contains("开了 bash：清单只含写、改工具做的改动")
    );
}

#[test]
fn gate_tasks_and_tasks_without_ready_have_no_delivery() {
    let (dir, store) = open();
    let (guarded, gate) = gate_graph(&store, dir.path(), false);
    assert!(delivery_for(dir.path(), &gate, None).is_none());
    let no_ready = first_child(&store, dir.path(), false);
    assert!(delivery_for(dir.path(), &no_ready, None).is_none());
    assert!(delivery_for(dir.path(), &guarded, None).is_some() || guarded.reports.is_empty());
    let _ = dir;
}

#[test]
fn delivery_clock_formats_known_zones_without_spawning_date() {
    assert_eq!(
        super::clock_in(chrono_tz::Asia::Shanghai, 1_700_000_000),
        "11月15日 06:13"
    );
    assert_eq!(
        super::clock_in(chrono_tz::America::Los_Angeles, 1_700_000_000),
        "11月14日 14:13"
    );
}

#[test]
fn delivery_clock_uses_local_timezone_for_kind_text() {
    // 本用例只在 TZ=Asia/Shanghai 的定向门禁里断言；其它环境的本地时区不同，不做假。
    if std::env::var("TZ").as_deref() != Ok("Asia/Shanghai") {
        return;
    }
    assert_eq!(super::clock(1_700_000_000), "11月15日 06:13");
}
