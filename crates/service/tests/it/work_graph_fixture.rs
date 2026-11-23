//! 任务图 e2e 夹具的重生成入口：三张已确认图 + 一个单派任务。
//!
//! 只给手工重生成用（#[ignore]）；写盘后由 normalize-task-graphs.mjs 做确定性 id 重映射，
//! 同一构造每次重生成的字节一致。命令见 web/e2e/fixtures/generate-task-graphs.sh。

use std::path::Path;

use tasks::{
    AttemptEnd, AttemptTrigger, ConversationId, EffectKind, GateCandidate, GateConclusion, Handoff,
    ModelChoice, NewGraph, NewReport, NewSubtask, NewTask, ReportOrigin, ReportState, Store, Task,
    TaskPolicy,
};

const T0: i64 = 1_790_000_000_000;

fn draft(cwd: &Path, title: &str) -> NewTask {
    NewTask {
        project_id: None,
        cwd_source: None,
        title: title.to_owned(),
        requirement: format!("{title}：要做到可核对"),
        dod: format!("{title} 的 DoD"),
        cwd: cwd.to_path_buf(),
        conversation: ConversationId::new("kind-work-graph-fixture"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn policy(write_scope: Option<&str>) -> TaskPolicy {
    TaskPolicy {
        write_scope: write_scope.map(str::to_owned),
        worktree: false,
        bash: false,
    }
}

fn report(state: ReportState, summary: &str) -> NewReport {
    NewReport {
        state,
        origin: ReportOrigin::Agent,
        summary: summary.to_owned(),
        question: None,
    }
}

fn drain_launches(store: &Store, now: i64) {
    while let Some(claim) = store.claim_effect(EffectKind::Launch, now).unwrap() {
        store.complete_effect(&claim).unwrap();
    }
}

fn begin(store: &Store, task: &Task, at: i64) -> tasks::AttemptId {
    let handoffs = store.current_handoffs(task).unwrap();
    store
        .begin_attempt_with(&task.id, at, AttemptTrigger::Launch, handoffs)
        .unwrap()
}

fn finish_ready(store: &Store, task: &Task, body: &str, at: i64) {
    let attempt = begin(store, task, at);
    store
        .record_report(&task.id, attempt, report(ReportState::Ready, body), at + 1)
        .unwrap();
    store
        .end_attempt(&task.id, attempt, at + 2, AttemptEnd::Reported)
        .unwrap();
}

fn subtask(cwd: &Path, title: &str, after: Vec<usize>, milestone: bool) -> NewSubtask {
    NewSubtask {
        task: draft(cwd, title),
        policy: policy(None),
        after,
        milestone,
        program_gate: Vec::new(),
    }
}

fn gate_until_judge(store: &Store, guarded: &Task, gate: &Task, at: i64) -> Task {
    finish_ready(store, guarded, "查好了，交候选", at);
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let latest = guarded.latest_report().cloned().unwrap();
    drain_launches(store, at + 3);
    let attempt = store
        .begin_attempt_with(
            &gate.id,
            at + 4,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: latest.id.clone(),
                    requirement_version: latest.version,
                },
            },
            vec![Handoff {
                report_id: latest.id.clone(),
                requirement_version: latest.version,
                summary: latest.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            attempt,
            report(ReportState::Ready, "核完了，等你判"),
            at + 5,
        )
        .unwrap();
    store
        .end_attempt(&gate.id, attempt, at + 6, AttemptEnd::Reported)
        .unwrap();
    store.task(&gate.id).unwrap().unwrap()
}

#[test]
#[ignore = "写 web/e2e/fixtures/t062-task-graphs.sqlite；重生成任务图夹具时手动跑"]
fn write_task_graphs_fixture() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = Path::new("/tmp");
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();

    // 图一：有门等你判（A 被门 G 把关，B 等 A）。
    let root = store
        .create_graph(
            NewGraph {
                root: draft(cwd, "有门等你判的图"),
                subtasks: vec![
                    subtask(cwd, "查行数", vec![], false),
                    subtask(cwd, "独立核查", vec![0], true),
                    subtask(cwd, "写结论", vec![0], false),
                ],
            },
            T0,
        )
        .unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    drain_launches(&store, T0 + 2);
    let children = store.children(&root.id).unwrap();
    gate_until_judge(&store, &children[0], &children[1], T0 + 10);

    // 图二：两个上游汇到一个下游；门的通过把 A 的交接写进 C，B 还在等。
    let root = store
        .create_graph(
            NewGraph {
                root: draft(cwd, "两个上游汇到一个下游的图"),
                subtasks: vec![
                    subtask(cwd, "查 A", vec![], false),
                    subtask(cwd, "核查 A", vec![0], true),
                    subtask(cwd, "查 B", vec![], false),
                    subtask(cwd, "写 C", vec![0, 2], false),
                ],
            },
            T0 + 100,
        )
        .unwrap();
    store.confirm_graph(&root.id, T0 + 101).unwrap();
    drain_launches(&store, T0 + 102);
    let children = store.children(&root.id).unwrap();
    let gate = gate_until_judge(&store, &children[0], &children[1], T0 + 110);
    let gate_report = gate.latest_report().cloned().unwrap();
    store
        .judge_gate(
            &gate.id,
            &gate_report.id,
            GateConclusion::Pass,
            None,
            T0 + 120,
        )
        .unwrap();
    drain_launches(&store, T0 + 121);

    // 图三：两个任务依次跑完并接受，全部关闭。
    let root = store
        .create_graph(
            NewGraph {
                root: draft(cwd, "全部关了的图"),
                subtasks: vec![
                    subtask(cwd, "第一步", vec![], false),
                    subtask(cwd, "第二步", vec![0], false),
                ],
            },
            T0 + 200,
        )
        .unwrap();
    store.confirm_graph(&root.id, T0 + 201).unwrap();
    drain_launches(&store, T0 + 202);
    let children = store.children(&root.id).unwrap();
    finish_ready(&store, &children[0], "第一步做完", T0 + 210);
    let first = store.task(&children[0].id).unwrap().unwrap();
    let first_report = first.latest_report().cloned().unwrap();
    store.accept(&first.id, &first_report.id, T0 + 214).unwrap();
    drain_launches(&store, T0 + 215);
    let second = store.task(&children[1].id).unwrap().unwrap();
    finish_ready(&store, &second, "第二步做完", T0 + 216);
    let second = store.task(&children[1].id).unwrap().unwrap();
    let second_report = second.latest_report().cloned().unwrap();
    store
        .accept(&second.id, &second_report.id, T0 + 220)
        .unwrap();

    // 单派任务一个（页尾计数）。
    let single = store
        .create_task(draft(cwd, "单派的一个任务"), T0 + 300)
        .unwrap();
    drain_launches(&store, T0 + 301);
    let single = store.task(&single.id).unwrap().unwrap();
    finish_ready(&store, &single, "单派做完了", T0 + 302);

    drop(store);
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../web/e2e/fixtures/t062-task-graphs.sqlite");
    std::fs::copy(directory.path().join("tasks.sqlite"), &fixtures).unwrap();
    println!("wrote {}", fixtures.display());
}
