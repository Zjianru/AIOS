//! T055.2：组接受（一个事务、复用单项校验、没通过只回原因、门跟关）。

use std::path::Path;

use crate::{
    AttemptEnd, AttemptTrigger, ConversationId, GateCandidate, GateConclusion, Handoff,
    ModelChoice, NewGraph, NewReport, NewSubtask, NewTask, ReportOrigin, ReportState, Stage, Store,
    Task, TaskId, TaskPolicy,
};
use tempfile::TempDir;

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
        conversation: ConversationId::new("kind-accept-many"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn policy() -> TaskPolicy {
    TaskPolicy {
        write_scope: None,
        worktree: false,
        bash: false,
    }
}

fn report(summary: &str) -> NewReport {
    NewReport {
        state: ReportState::Ready,
        origin: ReportOrigin::Agent,
        summary: summary.to_owned(),
        question: None,
    }
}

fn start(store: &Store, task: &Task, now: i64) -> crate::AttemptId {
    let claim = store.claim_effect(crate::EffectKind::Launch, now).unwrap();
    if let Some(claim) = claim {
        store.complete_effect(&claim).unwrap();
    }
    store.begin_attempt(&task.id, now).unwrap()
}

fn ready_task(store: &Store, cwd: &Path, title: &str, at: i64) -> Task {
    let task = store.create_task(draft(cwd, title), at).unwrap();
    let attempt = start(store, &task, at + 1);
    store
        .record_report(&task.id, attempt, report("查好了"), at + 2)
        .unwrap();
    store
        .end_attempt(&task.id, attempt, at + 3, AttemptEnd::Reported)
        .unwrap();
    store.task(&task.id).unwrap().unwrap()
}

fn report_id(task: &Task) -> crate::ReportId {
    task.latest_report().unwrap().id.clone()
}

fn milestone(cwd: &Path, title: &str, after: Vec<usize>) -> NewSubtask {
    NewSubtask {
        task: draft(cwd, title),
        policy: policy(),
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
                policy: policy(),
                after: vec![],
                milestone: false,
            },
            milestone(cwd, "独立核查", vec![0]),
            NewSubtask {
                task: draft(cwd, "再写结果"),
                policy: policy(),
                after: vec![0],
                milestone: false,
            },
        ],
    }
}

/// 被把关任务交了一份 ready，门也跑完 ready 但还没判：返回 (guarded, gate, 两份报告, root)。
fn gated_ready(store: &Store, cwd: &Path) -> (Task, Task, Task, crate::ReportId, crate::ReportId) {
    let root = store.create_graph(gated_graph(cwd), T0 + 1).unwrap();
    store.confirm_graph(&root.id, T0 + 2).unwrap();
    let children = store.children(&root.id).unwrap();
    let (guarded, gate) = (children[0].clone(), children[1].clone());

    let guarded_attempt = start(store, &guarded, T0 + 3);
    store
        .record_report(&guarded.id, guarded_attempt, report("查好了"), T0 + 4)
        .unwrap();
    let guarded = store.task(&guarded.id).unwrap().unwrap();
    let guarded_report = report_id(&guarded);

    let gate_claim = store
        .claim_effect(crate::EffectKind::Launch, T0 + 5)
        .unwrap()
        .unwrap();
    store.complete_effect(&gate_claim).unwrap();
    let gate_attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 6,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: guarded_report.clone(),
                    requirement_version: 1,
                },
            },
            vec![Handoff {
                report_id: guarded_report.clone(),
                requirement_version: 1,
                summary: "查好了".to_owned(),
            }],
        )
        .unwrap();
    store
        .record_report(&gate.id, gate_attempt, report("核查完了"), T0 + 7)
        .unwrap();
    let gate = store.task(&gate.id).unwrap().unwrap();
    (
        guarded,
        gate.clone(),
        root,
        guarded_report,
        report_id(&gate),
    )
}

fn judge_pass(store: &Store, gate: &Task, gate_report: &crate::ReportId, at: i64) {
    store
        .judge_gate(&gate.id, gate_report, GateConclusion::Pass, None, at)
        .unwrap();
}

#[test]
fn two_ready_reports_are_accepted_together() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let left = ready_task(&store, cwd, "写左边", T0);
    let right = ready_task(&store, cwd, "写右边", T0 + 10);
    let left_report = report_id(&left);
    let right_report = report_id(&right);

    let batch = store
        .accept_many(
            &[
                (left.id.clone(), left_report),
                (right.id.clone(), right_report),
            ],
            T0 + 20,
        )
        .unwrap();
    assert_eq!(batch.accepted_at, T0 + 20);
    assert_eq!(batch.outcomes.len(), 2);
    assert!(
        batch
            .outcomes
            .iter()
            .all(|outcome| outcome.refused.is_none())
    );
    assert_eq!(batch.outcomes[0].title, "写左边");
    assert_eq!(batch.outcomes[1].title, "写右边");
    for id in [&left.id, &right.id] {
        let task = store.task(id).unwrap().unwrap();
        assert_eq!(task.acceptances.len(), 1);
        assert_eq!(task.acceptances[0].at, T0 + 20);
        assert_eq!(store.stage(id).unwrap(), Stage::Closed);
        assert_eq!(store.closed_at(id).unwrap(), Some(T0 + 20));
    }
}

#[test]
fn a_stale_report_is_refused_while_another_is_accepted() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let stale = ready_task(&store, cwd, "旧结果", T0);
    let old_report = report_id(&stale);
    // 旧任务再开一次 Attempt，交一份新结果：旧报告不是最新了。
    let second = start(&store, &stale, T0 + 4);
    store
        .record_report(&stale.id, second, report("新结果"), T0 + 5)
        .unwrap();
    store
        .end_attempt(&stale.id, second, T0 + 6, AttemptEnd::Reported)
        .unwrap();
    let fresh = ready_task(&store, cwd, "新结果任务", T0 + 10);
    let fresh_report = report_id(&fresh);

    let batch = store
        .accept_many(
            &[
                (stale.id.clone(), old_report),
                (fresh.id.clone(), fresh_report),
            ],
            T0 + 20,
        )
        .unwrap();
    assert_eq!(batch.outcomes.len(), 2);
    assert!(
        batch.outcomes[0]
            .refused
            .as_deref()
            .is_some_and(|reason| reason.contains("已经变了")),
        "{:?}",
        batch.outcomes[0].refused
    );
    assert!(batch.outcomes[1].refused.is_none());
    assert!(
        store
            .task(&stale.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
    assert_eq!(store.task(&fresh.id).unwrap().unwrap().acceptances.len(), 1);
}

#[test]
fn an_old_requirement_version_is_refused_with_the_version_message() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = ready_task(&store, cwd, "改过要求", T0);
    let old_report = report_id(&task);
    store
        .update_requirement(&task.id, "新要求", Some("新 DoD"), T0 + 5)
        .unwrap();

    let batch = store
        .accept_many(&[(task.id.clone(), old_report)], T0 + 10)
        .unwrap();
    let refused = batch.outcomes[0].refused.as_deref().unwrap_or_default();
    assert!(refused.contains("要求改过了"), "{refused}");
    assert!(refused.contains("第 1 版"), "{refused}");
    assert!(
        store
            .task(&task.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
}

#[test]
fn the_gate_task_and_an_unjudged_guarded_result_are_both_refused() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (guarded, gate, _root, guarded_report, gate_report) = gated_ready(&store, cwd);

    let batch = store
        .accept_many(
            &[
                (gate.id.clone(), gate_report),
                (guarded.id.clone(), guarded_report),
            ],
            T0 + 20,
        )
        .unwrap();
    assert!(
        batch.outcomes[0]
            .refused
            .as_deref()
            .is_some_and(|reason| reason.contains("门任务")),
        "{:?}",
        batch.outcomes[0].refused
    );
    assert!(
        batch.outcomes[1]
            .refused
            .as_deref()
            .is_some_and(|reason| reason.contains("核查还没判通过")),
        "{:?}",
        batch.outcomes[1].refused
    );
    assert!(
        store
            .task(&gate.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
    assert!(
        store
            .task(&guarded.id)
            .unwrap()
            .unwrap()
            .acceptances
            .is_empty()
    );
}

#[test]
fn accepting_the_same_report_twice_is_idempotent() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = ready_task(&store, cwd, "幂等", T0);
    let report_id = report_id(&task);

    let first = store
        .accept_many(&[(task.id.clone(), report_id.clone())], T0 + 10)
        .unwrap();
    assert!(first.outcomes[0].refused.is_none());
    let second = store
        .accept_many(&[(task.id.clone(), report_id)], T0 + 20)
        .unwrap();
    assert!(second.outcomes[0].refused.is_none());
    let task = store.task(&task.id).unwrap().unwrap();
    assert_eq!(task.acceptances.len(), 1, "同一份结果只记一次");
    assert_eq!(task.acceptances[0].at, T0 + 10, "时间不改");
}

#[test]
fn accepting_a_guarded_result_also_closes_the_gate() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (guarded, gate, _root, guarded_report, gate_report) = gated_ready(&store, cwd);
    judge_pass(&store, &gate, &gate_report, T0 + 8);

    let batch = store
        .accept_many(&[(guarded.id.clone(), guarded_report)], T0 + 10)
        .unwrap();
    assert!(batch.outcomes[0].refused.is_none());
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::Closed);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::Closed);
    assert_eq!(store.closed_at(&guarded.id).unwrap(), Some(T0 + 10));
    assert_eq!(store.closed_at(&gate.id).unwrap(), Some(T0 + 10));
}

#[test]
fn a_missing_task_is_refused_without_failing_the_rest() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = ready_task(&store, cwd, "在的", T0);
    let report_id = report_id(&task);
    let missing = TaskId::from("00000000000000000000000000000000");

    let batch = store
        .accept_many(
            &[
                (missing.clone(), report_id.clone()),
                (task.id.clone(), report_id),
            ],
            T0 + 10,
        )
        .unwrap();
    assert!(batch.outcomes[0].refused.is_some());
    assert!(batch.outcomes[1].refused.is_none());
    assert!(store.task(&missing).unwrap().is_none());
    assert_eq!(store.task(&task.id).unwrap().unwrap().acceptances.len(), 1);
}
