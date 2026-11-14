//! T055：任务列表与关闭时间（看板/「现在」的数据底）。

use std::path::Path;

use tempfile::TempDir;

use crate::{
    AttemptEnd, AttemptTrigger, ConversationId, EffectKind, GateCandidate, GateConclusion, Handoff,
    ModelChoice, NewGraph, NewReport, NewSubtask, NewTask, ReportOrigin, ReportState, Store,
    TaskPolicy,
};

const T0: i64 = 1_700_000_000_000;

fn open_store() -> (TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn draft(cwd: &Path, title: &str) -> NewTask {
    NewTask {
        title: title.to_owned(),
        requirement: format!("{title} 的要求"),
        dod: format!("{title} 的 DoD"),
        cwd: cwd.to_path_buf(),
        conversation: ConversationId::new("kind-work"),
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

fn ready_report() -> NewReport {
    NewReport {
        state: ReportState::Ready,
        origin: ReportOrigin::Agent,
        summary: "查好了".to_owned(),
        question: None,
    }
}

fn milestone(cwd: &Path, title: &str, after: Vec<usize>) -> NewSubtask {
    NewSubtask {
        task: draft(cwd, title),
        policy: policy(None),
        after,
        milestone: true,
    }
}

fn gated_graph(cwd: &Path) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            NewSubtask {
                task: draft(cwd, "先查行数"),
                policy: policy(None),
                after: vec![],
                milestone: false,
            },
            milestone(cwd, "独立核查", vec![0]),
            NewSubtask {
                task: draft(cwd, "再写结果"),
                policy: policy(None),
                after: vec![0],
                milestone: false,
            },
        ],
    }
}

#[test]
fn all_tasks_is_ordered_by_created_at() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let late = store.create_task(draft(cwd, "晚一点"), T0 + 20).unwrap();
    let early = store.create_task(draft(cwd, "早一点"), T0).unwrap();
    let middle = store.create_task(draft(cwd, "中间"), T0 + 10).unwrap();
    let all = store.all_tasks().unwrap();
    let ids: Vec<&str> = all.iter().map(|task| task.id.as_str()).collect();
    assert_eq!(
        ids,
        vec![early.id.as_str(), middle.id.as_str(), late.id.as_str()]
    );
    assert_eq!(store.task(&middle.id).unwrap().unwrap().title, "中间");
}

#[test]
fn closed_at_is_the_acceptance_time_for_a_plain_task() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = store.create_task(draft(cwd, "查一下"), T0).unwrap();
    assert_eq!(store.closed_at(&task.id).unwrap(), None, "还没结果");

    let claim = store.claim_effect(EffectKind::Launch, T0 + 1).unwrap();
    store.complete_effect(&claim.unwrap()).unwrap();
    let attempt = store.begin_attempt(&task.id, T0 + 2).unwrap();
    let report = store
        .record_report(&task.id, attempt, ready_report(), T0 + 3)
        .unwrap();
    store
        .end_attempt(&task.id, attempt, T0 + 4, AttemptEnd::Reported)
        .unwrap();
    assert_eq!(store.closed_at(&task.id).unwrap(), None, "接受前没关闭");

    let acceptance = store.accept(&task.id, &report, T0 + 5).unwrap();
    assert_eq!(acceptance.at, T0 + 5);
    assert_eq!(store.closed_at(&task.id).unwrap(), Some(T0 + 5));
}

#[test]
fn closed_at_follows_the_guarded_task_for_a_gate_and_is_none_for_the_root() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    let root = store.create_graph(gated_graph(cwd), T0).unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let children = store.children(&root.id).unwrap();
    let (guarded, gate, downstream) = (
        children[0].clone(),
        children[1].clone(),
        children[2].clone(),
    );

    // 被把关任务先报一份 ready，门跑完并判通过。
    let claim = store.claim_effect(EffectKind::Launch, T0 + 2).unwrap();
    store.complete_effect(&claim.unwrap()).unwrap();
    let b1 = store.begin_attempt(&guarded.id, T0 + 3).unwrap();
    store
        .record_report(&guarded.id, b1, ready_report(), T0 + 4)
        .unwrap();
    let b_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let claim = store.claim_effect(EffectKind::Launch, T0 + 5).unwrap();
    store.complete_effect(&claim.unwrap()).unwrap();
    let gate_attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 6,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: b_report.id.clone(),
                    requirement_version: b_report.version,
                },
            },
            vec![Handoff {
                report_id: b_report.id.clone(),
                requirement_version: b_report.version,
                summary: b_report.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(&gate.id, gate_attempt, ready_report(), T0 + 7)
        .unwrap();
    store
        .end_attempt(&gate.id, gate_attempt, T0 + 7, AttemptEnd::Reported)
        .unwrap();
    let gate_report = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    store
        .judge_gate(
            &gate.id,
            &gate_report.id,
            GateConclusion::Pass,
            None,
            T0 + 8,
        )
        .unwrap();

    // 接受之前都没有 closed_at。
    assert_eq!(store.closed_at(&guarded.id).unwrap(), None);
    assert_eq!(store.closed_at(&gate.id).unwrap(), None);
    assert_eq!(store.closed_at(&root.id).unwrap(), None);

    let acceptance = store.accept(&guarded.id, &b_report.id, T0 + 9).unwrap();
    assert_eq!(acceptance.at, T0 + 9);
    assert_eq!(store.closed_at(&guarded.id).unwrap(), Some(T0 + 9));
    assert_eq!(
        store.closed_at(&gate.id).unwrap(),
        Some(T0 + 9),
        "门任务跟被把关任务"
    );
    assert_eq!(store.closed_at(&downstream.id).unwrap(), None);
    assert_eq!(
        store.closed_at(&root.id).unwrap(),
        None,
        "总任务不给关闭时间"
    );
}

#[test]
fn all_tasks_sees_graph_roots_and_children() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    let root = store.create_graph(gated_graph(cwd), T0).unwrap();
    let all = store.all_tasks().unwrap();
    assert!(all.iter().any(|task| task.id == root.id));
    assert_eq!(all.len(), 4);
}
