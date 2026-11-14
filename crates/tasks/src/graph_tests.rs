//! T052：任务图的建立、确认、阶段汇总与总任务不跑执行。

use std::path::Path;

use tempfile::TempDir;

use crate::{
    AttemptEnd, AttemptTrigger, CancellationReason, ConversationId, EffectKind, EffectPayload,
    GateCandidate, GateConclusion, Handoff, ModelChoice, NewGraph, NewReport, NewSubtask, NewTask,
    ReportOrigin, ReportState, Stage, Store, Task, TaskError, TaskPolicy,
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
        conversation: ConversationId::new("kind-graph"),
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

fn subtask(cwd: &Path, title: &str, write_scope: Option<&str>, after: Vec<usize>) -> NewSubtask {
    NewSubtask {
        task: draft(cwd, title),
        policy: policy(write_scope),
        after,
        milestone: false,
    }
}

/// 两项的图：A 不等人，B 等 A（或者按参数不等待）。
fn two_item_graph(cwd: &Path, scope_a: Option<&str>, scope_b: Option<&str>) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            subtask(cwd, "先查行数", scope_a, vec![]),
            subtask(cwd, "再写结果", scope_b, vec![0]),
        ],
    }
}

fn create_two(store: &Store, cwd: &Path) -> (Task, Vec<Task>) {
    let root = store
        .create_graph(two_item_graph(cwd, Some("notes.md"), Some("other.txt")), T0)
        .unwrap();
    let children = store.children(&root.id).unwrap();
    (root, children)
}

fn record_ready(store: &Store, task: &Task, summary: &str) {
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: summary.to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
}

#[test]
fn graph_validation_rejects_bad_shapes() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let root = draft(cwd, "总任务");

    let too_few = NewGraph {
        root: root.clone(),
        subtasks: vec![subtask(cwd, "只有一项", None, vec![])],
    };
    assert!(matches!(
        store.create_graph(too_few, T0),
        Err(TaskError::GraphTooFewSubtasks(1))
    ));

    let out_of_range = NewGraph {
        root: root.clone(),
        subtasks: vec![
            subtask(cwd, "A", None, vec![]),
            subtask(cwd, "B", None, vec![9]),
        ],
    };
    assert!(matches!(
        store.create_graph(out_of_range, T0),
        Err(TaskError::GraphAfterOutOfRange {
            subtask: 1,
            after: 9
        })
    ));

    let cycle = NewGraph {
        root: root.clone(),
        subtasks: vec![
            subtask(cwd, "A", None, vec![1]),
            subtask(cwd, "B", None, vec![0]),
        ],
    };
    assert!(matches!(
        store.create_graph(cycle, T0),
        Err(TaskError::GraphHasCycle)
    ));
}

#[test]
fn graph_validation_rejects_overlapping_write_scopes_without_ordering() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    // 两项都会先跑：写入范围重叠（D15 单写者）必须拒绝。
    let overlapping = two_item_graph(cwd, Some("notes.md"), Some("notes.md"));
    let overlapping = NewGraph {
        root: overlapping.root,
        subtasks: vec![
            subtask(cwd, "A", Some("notes.md"), vec![]),
            subtask(cwd, "B", Some("notes.md"), vec![]),
        ],
    };
    assert!(matches!(
        store.create_graph(overlapping, T0),
        Err(TaskError::GraphWriteScopeOverlap { a: 0, b: 1, .. })
    ));
    // 有先后就可以用同一个范围：不会同时跑。
    let ordered = store
        .create_graph(two_item_graph(cwd, Some("notes.md"), Some("notes.md")), T0)
        .expect("有先后的重叠范围可以建图");
    assert!(ordered.is_graph_root());
}

#[test]
fn create_graph_persists_the_whole_graph_without_launching_it() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);

    assert!(root.is_graph_root());
    let arrangement = root.arrangement.clone().expect("总任务有安排记录");
    assert_eq!(arrangement.version, 1);
    assert!(arrangement.confirmed.is_none(), "建图不等于确认");
    assert_eq!(store.task_ids().unwrap().len(), 3, "总任务 + 两个子任务");
    assert_eq!(children.len(), 2);
    assert_eq!(children[0].parent.as_ref(), Some(&root.id));
    assert!(children[0].needs.is_empty());
    assert_eq!(children[1].needs.len(), 1);
    assert_eq!(children[1].needs[0].from, children[0].id);
    assert!(children[1].needs[0].handoff.is_none(), "开跑时才解析交接");

    // 图建好不等于开跑：没有任何 launch 效果行。
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 10)
            .unwrap()
            .is_none()
    );
    // 总任务没有执行承载。
    assert!(matches!(
        store.begin_attempt(&root.id, T0 + 11),
        Err(TaskError::GraphRootCannotRun)
    ));
}

#[test]
fn confirm_graph_launches_ready_children_exactly_once() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);

    assert_eq!(store.stage(&root.id).unwrap(), Stage::NeedsConditions);
    assert_eq!(
        store.stage(&children[0].id).unwrap(),
        Stage::NeedsConditions
    );
    assert_eq!(
        store.stage(&children[1].id).unwrap(),
        Stage::NeedsConditions
    );

    let launched = store.confirm_graph(&root.id, T0 + 1).unwrap();
    assert_eq!(launched, 1, "只放没有 needs 的第一项");
    assert_eq!(store.stage(&children[0].id).unwrap(), Stage::Pending);
    assert_eq!(
        store.stage(&children[1].id).unwrap(),
        Stage::NeedsConditions,
        "等上游的子任务还是待补足条件"
    );
    assert_eq!(
        store.stage(&root.id).unwrap(),
        Stage::NeedsConditions,
        "总任务取最靠前的子任务阶段"
    );

    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A 的 launch");
    let EffectPayload::Launch { task_id } = claim.payload.clone() else {
        panic!("launch 载荷");
    };
    assert_eq!(task_id, children[0].id);
    store.complete_effect(&claim).unwrap();
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 3)
            .unwrap()
            .is_none(),
        "没有 needs 的只放一次"
    );
    assert!(matches!(
        store.confirm_graph(&root.id, T0 + 4),
        Err(TaskError::GraphAlreadyConfirmed)
    ));
}

#[test]
fn stage_rolls_up_children_and_waits_for_upstream_ready() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A 的 launch");
    store.complete_effect(&claim).unwrap();
    record_ready(&store, &children[0], "查完了：notes.md 有 3 行");

    assert_eq!(store.stage(&children[0].id).unwrap(), Stage::ReadyForReview);
    assert_eq!(
        store.stage(&children[1].id).unwrap(),
        Stage::Pending,
        "上游 ready 后，下游从待补足条件变待执行"
    );
    assert_eq!(store.stage(&root.id).unwrap(), Stage::Pending, "总任务汇总");

    record_ready(&store, &children[1], "写好了");
    assert_eq!(store.stage(&children[1].id).unwrap(), Stage::ReadyForReview);
    assert_eq!(
        store.stage(&root.id).unwrap(),
        Stage::ReadyForReview,
        "子任务全到待检查，总任务才待检查"
    );
}

#[test]
fn old_task_rows_without_graph_fields_read_as_single_tasks() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let task = store.create_task(draft(cwd, "单派任务"), T0).unwrap();
    let mut value = serde_json::to_value(&task).unwrap();
    let object = value.as_object_mut().unwrap();
    object.remove("parent");
    object.remove("needs");
    object.remove("arrangement");
    let old: Task = serde_json::from_value(value).unwrap();
    assert!(old.parent.is_none());
    assert!(old.needs.is_empty());
    assert!(old.arrangement.is_none());
    assert!(!old.is_graph_root());
}

#[test]
fn ready_report_releases_downstream_with_handoff_exactly_once() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    // A 的 launch 排上了；收掉它，模拟 A 已经跑完（报告走 record_report 的同一事务放出逻辑）。
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    record_ready(&store, &children[0], "查完了：notes.md 有 3 行");

    let downstream = store.task(&children[1].id).unwrap().unwrap();
    let handoff = downstream.needs[0].handoff.clone().expect("开跑前解析交接");
    let upstream = store.task(&children[0].id).unwrap().unwrap();
    let report = upstream.latest_report().expect("A 的 ready 报告");
    assert_eq!(handoff.report_id, report.id, "交接绑上游报告");
    assert_eq!(handoff.requirement_version, report.version);
    assert!(handoff.summary.contains("3 行"), "交接带摘要：{handoff:?}");

    // 下游的 launch 只排了一条；relay 里带「已自动开跑」的标题。
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 3)
        .unwrap()
        .expect("B launch");
    let EffectPayload::Launch { task_id } = claim.payload.clone() else {
        panic!("launch 载荷");
    };
    assert_eq!(task_id, children[1].id);
    store.complete_effect(&claim).unwrap();
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 4)
            .unwrap()
            .is_none(),
        "只放一次"
    );
    let relay = store
        .claim_effect(EffectKind::Relay, T0 + 5)
        .unwrap()
        .expect("A 的 relay");
    let EffectPayload::Relay { released, .. } = relay.payload.clone() else {
        panic!("relay 载荷");
    };
    assert_eq!(released, vec!["再写结果".to_owned()]);
}

#[test]
fn failed_report_does_not_release_downstream() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    let attempt = store.begin_attempt(&children[0].id, T0 + 3).unwrap();
    store
        .record_report(
            &children[0].id,
            attempt,
            NewReport {
                state: ReportState::Failed,
                origin: ReportOrigin::Agent,
                summary: "跑挂了".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    let downstream = store.task(&children[1].id).unwrap().unwrap();
    assert!(downstream.needs[0].handoff.is_none(), "失败不写交接");
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 6)
            .unwrap()
            .is_none(),
        "失败不放下游"
    );
}

#[test]
fn boot_scan_interrupts_running_child_but_not_graph_root_or_waiting_sibling() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    let attempt = store.begin_attempt(&children[0].id, T0 + 3).unwrap();
    store
        .attach_session(
            &children[0].id,
            attempt,
            ConversationId::new("task-a-session"),
        )
        .unwrap();

    let scan = store
        .scan_at_boot(&crate::BootId::new("boot-graph"), T0 + 10)
        .unwrap();
    assert_eq!(
        scan.interrupted,
        vec![children[0].id.clone()],
        "只有真正在跑的子任务算被打断"
    );
    let running = store.task(&children[0].id).unwrap().unwrap();
    assert_eq!(
        running.latest_attempt().and_then(|attempt| attempt.end),
        Some(AttemptEnd::Interrupted)
    );
    let waiting = store.task(&children[1].id).unwrap().unwrap();
    assert!(waiting.latest_attempt().is_none(), "B 没开跑，不出被打断");
    assert!(!waiting.is_interrupted());
    let total = store.task(&root.id).unwrap().unwrap();
    assert!(total.latest_attempt().is_none(), "总任务没有 Attempt");
    assert!(!total.is_interrupted());
    assert_eq!(
        store.stage(&children[1].id).unwrap(),
        Stage::NeedsConditions
    );
    assert_eq!(store.stage(&root.id).unwrap(), Stage::NeedsConditions);
}

#[test]
fn crash_after_ready_release_keeps_downstream_launch_queued_exactly_once() {
    let (directory, store) = open_store();
    let path = directory.path().join("tasks.sqlite");
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    let attempt = store.begin_attempt(&children[0].id, T0 + 3).unwrap();
    store
        .attach_session(
            &children[0].id,
            attempt,
            ConversationId::new("task-a-session"),
        )
        .unwrap();
    store
        .record_report(
            &children[0].id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "查完了：notes.md 有 3 行".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    // 模拟崩溃：直接丢掉连接、重新打开同一个库。
    drop(store);
    let store = Store::open(&path).unwrap();

    // A 的 ready 报告和「放出 B」的 launch 都已经落库；重启扫描只把 A 记成 reported，不是 interrupted。
    let scan = store
        .scan_at_boot(&crate::BootId::new("boot-crash"), T0 + 20)
        .unwrap();
    assert!(scan.interrupted.is_empty(), "ready 的 A 不算被打断");
    assert_eq!(scan.reported, vec![children[0].id.clone()]);
    let waiting = store.task(&children[1].id).unwrap().unwrap();
    assert!(waiting.latest_attempt().is_none());
    assert!(waiting.needs[0].handoff.is_some(), "交接已持久化");

    // 重启后 B 的 launch 还在，恰好一条；收掉以后不会再排。
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 30)
        .unwrap()
        .expect("重启后 B 的 launch 还在");
    let EffectPayload::Launch { task_id } = claim.payload.clone() else {
        panic!("launch 载荷");
    };
    assert_eq!(task_id, children[1].id);
    store.complete_effect(&claim).unwrap();
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 31)
            .unwrap()
            .is_none(),
        "只放一次"
    );
}

#[test]
fn second_ready_updates_handoff_without_relaunching_or_repeating_released() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    // A 第一次 ready：放出 B。
    let a1 = store.begin_attempt(&children[0].id, T0 + 3).unwrap();
    store
        .record_report(
            &children[0].id,
            a1,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第一次结果".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 5)
        .unwrap()
        .expect("B launch");
    store.complete_effect(&claim).unwrap();
    let b1 = store.begin_attempt(&children[1].id, T0 + 6).unwrap();
    let _ = b1;
    // 收掉 A 第一次 ready 的 relay（带 released），再看第二次 ready。
    let relay = store
        .claim_effect(EffectKind::Relay, T0 + 7)
        .unwrap()
        .expect("A 第一次的 relay");
    store.complete_effect(&relay).unwrap();
    // A 第二次 ready：只刷新交接，不再放 B、不再重复说 released。
    store
        .end_attempt(&children[0].id, a1, T0 + 7, AttemptEnd::Reported)
        .unwrap();
    let a2 = store.begin_attempt(&children[0].id, T0 + 8).unwrap();
    store
        .record_report(
            &children[0].id,
            a2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第二次结果：3 行".to_owned(),
                question: None,
            },
            T0 + 9,
        )
        .unwrap();

    let downstream = store.task(&children[1].id).unwrap().unwrap();
    let handoff = downstream.needs[0].handoff.clone().expect("交接");
    assert!(
        handoff.summary.contains("第二次结果"),
        "交接刷新为最新 ready：{handoff:?}"
    );
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 12)
            .unwrap()
            .is_none(),
        "上游二次 ready 不能重新放 B"
    );
    let relay = store
        .claim_effect(EffectKind::Relay, T0 + 13)
        .unwrap()
        .expect("A 第二次的 relay");
    let EffectPayload::Relay { released, .. } = relay.payload.clone() else {
        panic!("relay 载荷");
    };
    assert!(released.is_empty(), "第二次不再说已自动开跑：{released:?}");
}

#[test]
fn blocked_report_does_not_release_downstream() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    let attempt = store.begin_attempt(&children[0].id, T0 + 3).unwrap();
    store
        .record_report(
            &children[0].id,
            attempt,
            NewReport {
                state: ReportState::Blocked,
                origin: ReportOrigin::Agent,
                summary: "卡住了".to_owned(),
                question: Some("怎么办".to_owned()),
            },
            T0 + 4,
        )
        .unwrap();
    let downstream = store.task(&children[1].id).unwrap().unwrap();
    assert!(downstream.needs[0].handoff.is_none(), "blocked 不写交接");
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 6)
            .unwrap()
            .is_none(),
        "blocked 不放下游"
    );
}

#[test]
fn stopped_attempt_does_not_release_downstream() {
    let (directory, store) = open_store();
    let cwd = directory.path();
    let (root, children) = create_two(&store, cwd);
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("A launch");
    store.complete_effect(&claim).unwrap();
    let attempt = store.begin_attempt(&children[0].id, T0 + 3).unwrap();
    store
        .stop_attempt(&children[0].id, attempt, T0 + 4)
        .unwrap();
    let downstream = store.task(&children[1].id).unwrap().unwrap();
    assert!(downstream.needs[0].handoff.is_none(), "stopped 不写交接");
    assert!(
        store
            .claim_effect(EffectKind::Launch, T0 + 6)
            .unwrap()
            .is_none(),
        "stopped 不放下游"
    );
}

// ===== T053.1 里程碑门 =====

fn milestone_subtask(cwd: &Path, title: &str, after: Vec<usize>) -> NewSubtask {
    NewSubtask {
        task: draft(cwd, title),
        policy: TaskPolicy::default(),
        after,
        milestone: true,
    }
}

/// 一个总任务 + 三个子任务：B（被把关）、门（after B）、C（after B）。
fn gated_graph(cwd: &Path) -> NewGraph {
    NewGraph {
        root: draft(cwd, "总任务"),
        subtasks: vec![
            subtask(cwd, "先查行数", None, vec![]),
            milestone_subtask(cwd, "独立核查", vec![0]),
            subtask(cwd, "再写结果", None, vec![0]),
        ],
    }
}

#[test]
fn milestone_validation_rejects_each_bad_shape() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    // 有写入范围。
    let mut graph = gated_graph(cwd);
    graph.subtasks[1].policy = policy(Some("other.txt"));
    assert!(matches!(
        store.create_graph(graph, T0),
        Err(TaskError::GraphMilestoneHasWriteScope { subtask: 1 })
    ));
    // 开工作树。
    let mut graph = gated_graph(cwd);
    graph.subtasks[1].policy.worktree = true;
    assert!(matches!(
        store.create_graph(graph, T0),
        Err(TaskError::GraphMilestoneHasWorktree { subtask: 1 })
    ));
    // after 不是恰好一项。
    let mut graph = gated_graph(cwd);
    graph.subtasks[1].after = vec![];
    assert!(matches!(
        store.create_graph(graph, T0),
        Err(TaskError::GraphMilestoneAfterNotOne {
            subtask: 1,
            count: 0
        })
    ));
    // 门链：门把另一个门当把关对象。
    let mut graph = gated_graph(cwd);
    graph
        .subtasks
        .push(milestone_subtask(cwd, "再核一遍", vec![1]));
    assert!(matches!(
        store.create_graph(graph, T0),
        Err(TaskError::GraphMilestoneAfterIsMilestone { subtask: 3 })
    ));
    // 两个门把关同一个任务。
    let mut graph = gated_graph(cwd);
    graph
        .subtasks
        .push(milestone_subtask(cwd, "另一个门", vec![0]));
    assert!(matches!(
        store.create_graph(graph, T0),
        Err(TaskError::GraphTwoMilestonesForGuarded {
            first: 1,
            second: 3
        })
    ));
    // 别人的 after 指向门。
    let mut graph = gated_graph(cwd);
    graph.subtasks.push(subtask(cwd, "等门", None, vec![1]));
    assert!(matches!(
        store.create_graph(graph, T0),
        Err(TaskError::GraphMilestoneCannotBeDependency { subtask: 3 })
    ));
    // 干净的图能建。
    let graph = gated_graph(cwd);
    let root = store.create_graph(graph, T0).unwrap();
    let children = store.children(&root.id).unwrap();
    assert!(children[1].is_gate());
    assert!(!children[0].is_gate());
}

#[test]
fn gate_pass_judgement_releases_guarded_downstream_and_stage_moves_to_requirement_met() {
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

    // 只有 B 没 needs：确认后先放 B，门和 C 都不开。
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 2)
        .unwrap()
        .expect("B launch");
    assert_eq!(
        claim.payload.clone(),
        EffectPayload::Launch {
            task_id: guarded.id.clone()
        }
    );
    store.complete_effect(&claim).unwrap();
    assert!(
        !store
            .effect_pending(EffectKind::Launch, gate.id.as_str())
            .unwrap()
    );
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );

    // B 报回 ready：门被放出，C 仍然不动。
    let attempt = store.begin_attempt(&guarded.id, T0 + 3).unwrap();
    store
        .record_report(
            &guarded.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第一次结果：3 行".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    assert!(
        store
            .effect_pending(EffectKind::Launch, gate.id.as_str())
            .unwrap()
    );
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );
    assert_eq!(
        store.stage(&downstream.id).unwrap(),
        Stage::NeedsConditions,
        "C 等门通过"
    );

    // 门开跑（交接 B 的候选），报回 ready：等用户判，C 还不开。
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 5)
        .unwrap()
        .expect("gate launch");
    store.complete_effect(&claim).unwrap();
    let b_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let handoff = Handoff {
        report_id: b_report.id.clone(),
        requirement_version: b_report.version,
        summary: b_report.summary.clone(),
    };
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
            vec![handoff.clone()],
        )
        .unwrap();
    let gate_task = store.task(&gate.id).unwrap().unwrap();
    assert_eq!(
        gate_task.latest_attempt().unwrap().handoffs,
        vec![handoff.clone()],
        "Attempt.handoffs 记开跑时拿到的上游报告"
    );
    store
        .record_report(
            &gate.id,
            gate_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "核查发现：空标题会漏判".to_owned(),
                question: None,
            },
            T0 + 7,
        )
        .unwrap();
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::ReadyForReview);
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::ReadyForReview);
    assert!(
        !store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );

    // 用户判通过：B 到任务要求已满足，C 被放出。
    let gate_report = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let outcome = store
        .judge_gate(
            &gate.id,
            &gate_report.id,
            GateConclusion::Pass,
            None,
            T0 + 8,
        )
        .unwrap();
    assert!(!outcome.duplicate);
    assert_eq!(outcome.released, vec!["再写结果".to_owned()]);
    assert_eq!(store.stage(&guarded.id).unwrap(), Stage::RequirementMet);
    assert_eq!(store.stage(&gate.id).unwrap(), Stage::RequirementMet);
    let c = store.task(&downstream.id).unwrap().unwrap();
    assert_eq!(
        c.needs[0]
            .handoff
            .as_ref()
            .map(|handoff| handoff.report_id.clone()),
        Some(b_report.id.clone()),
        "C 交接的是判通过的那份"
    );
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 9)
        .unwrap()
        .expect("C launch");
    store.complete_effect(&claim).unwrap();
    let c_attempt = store.begin_attempt(&downstream.id, T0 + 10).unwrap();
    store
        .record_report(
            &downstream.id,
            c_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "估完了".to_owned(),
                question: None,
            },
            T0 + 11,
        )
        .unwrap();
    assert_eq!(store.stage(&root.id).unwrap(), Stage::ReadyForReview);
}

#[test]
fn second_candidate_reruns_gate_and_old_gate_report_is_already_changed() {
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
    let claim = store.claim_effect(EffectKind::Launch, T0 + 2).unwrap();
    store.complete_effect(&claim.unwrap()).unwrap();
    let attempt = store.begin_attempt(&guarded.id, T0 + 3).unwrap();
    store
        .record_report(
            &guarded.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第一次结果".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 5)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let first_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let first_handoff = Handoff {
        report_id: first_report.id.clone(),
        requirement_version: first_report.version,
        summary: first_report.summary.clone(),
    };
    let gate_attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 6,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: first_report.id.clone(),
                    requirement_version: first_report.version,
                },
            },
            vec![first_handoff],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            gate_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第一次核查".to_owned(),
                question: None,
            },
            T0 + 7,
        )
        .unwrap();
    let first_gate_report = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    // 运行时记录报告后会把 Attempt 收尾；这里照做，否则门算「还在跑」。
    store
        .end_attempt(&gate.id, gate_attempt, T0 + 7, AttemptEnd::Reported)
        .unwrap();

    // 门已报回、还没判：B 又交了一份新结果 → 只记交接，门重跑一次用最新候选。
    store
        .end_attempt(&guarded.id, attempt, T0 + 8, AttemptEnd::Reported)
        .unwrap();
    let second_attempt = store.begin_attempt(&guarded.id, T0 + 9).unwrap();
    store
        .record_report(
            &guarded.id,
            second_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第二次结果：5 行".to_owned(),
                question: None,
            },
            T0 + 10,
        )
        .unwrap();
    let gate_now = store.task(&gate.id).unwrap().unwrap();
    assert_eq!(
        gate_now.needs[0].handoff.as_ref().unwrap().summary,
        "第二次结果：5 行"
    );
    assert!(
        store
            .effect_pending(EffectKind::Launch, gate.id.as_str())
            .unwrap()
    );

    // 旧门报告这时点判定：候选已经变了 → 已经变了，不记。
    assert!(matches!(
        store.judge_gate(
            &gate.id,
            &first_gate_report.id,
            GateConclusion::Pass,
            None,
            T0 + 11
        ),
        Err(TaskError::GateReportChanged)
    ));
    let gate_now = store.task(&gate.id).unwrap().unwrap();
    assert!(gate_now.verdicts.is_empty(), "已经变了不记判定");

    // 新候选的门重跑报回，判通过后 C 才开。
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 12)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let second_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let second_handoff = Handoff {
        report_id: second_report.id.clone(),
        requirement_version: second_report.version,
        summary: second_report.summary.clone(),
    };
    let recheck = store
        .begin_attempt_with(
            &gate.id,
            T0 + 13,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: second_report.id.clone(),
                    requirement_version: second_report.version,
                },
            },
            vec![second_handoff],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            recheck,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第二次核查".to_owned(),
                question: None,
            },
            T0 + 14,
        )
        .unwrap();
    let second_gate_report = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let outcome = store
        .judge_gate(
            &gate.id,
            &second_gate_report.id,
            GateConclusion::Pass,
            None,
            T0 + 15,
        )
        .unwrap();
    assert_eq!(outcome.released, vec!["再写结果".to_owned()]);
    assert!(
        store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );
    // 同一份门报告再判：返回已有判定，不记第二次。
    let again = store
        .judge_gate(
            &gate.id,
            &second_gate_report.id,
            GateConclusion::NeedsFix,
            None,
            T0 + 16,
        )
        .unwrap();
    assert!(again.duplicate);
    assert_eq!(again.verdict.conclusion, GateConclusion::Pass);
    assert_eq!(store.task(&gate.id).unwrap().unwrap().verdicts.len(), 1);
}

#[test]
fn propose_same_call_id_creates_one_graph() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    let (first, created) = store
        .create_graph_once(gated_graph(cwd), "call-graph-1", T0)
        .unwrap();
    assert!(created);
    let (second, created) = store
        .create_graph_once(gated_graph(cwd), "call-graph-1", T0 + 1)
        .unwrap();
    assert!(!created, "同一个 call id 返回同一张图");
    assert_eq!(first.id, second.id);
    assert_eq!(store.task_ids().unwrap().len(), 4, "只建了一张图");
}

#[test]
fn old_task_rows_without_gate_fields_read_with_defaults() {
    let directory = tempfile::tempdir().unwrap();
    let store = open_store().1;
    let task = store
        .create_task(draft(directory.path(), "旧行"), T0)
        .unwrap();
    let mut raw = serde_json::to_value(&task).unwrap();
    raw.as_object_mut().unwrap().remove("gate");
    raw.as_object_mut().unwrap().remove("verdicts");
    let parsed: Task = serde_json::from_value(raw).unwrap();
    assert!(parsed.gate.is_none());
    assert!(parsed.verdicts.is_empty());
    assert!(parsed.latest_attempt().is_none());
}

#[test]
fn new_candidate_while_gate_running_reruns_only_once_with_latest() {
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
    let claim = store.claim_effect(EffectKind::Launch, T0 + 2).unwrap();
    store.complete_effect(&claim.unwrap()).unwrap();
    // B 第一次交结果，门开跑（candidate1）。
    let b1 = store.begin_attempt(&guarded.id, T0 + 3).unwrap();
    store
        .record_report(
            &guarded.id,
            b1,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第一次结果".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 5)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let b1_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let handoff1 = Handoff {
        report_id: b1_report.id.clone(),
        requirement_version: b1_report.version,
        summary: b1_report.summary.clone(),
    };
    let gate_attempt = store
        .begin_attempt_with(
            &gate.id,
            T0 + 6,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: b1_report.id.clone(),
                    requirement_version: b1_report.version,
                },
            },
            vec![handoff1],
        )
        .unwrap();
    // 门在跑：B 又交一份新结果。不新排 launch，只把门上的交接换成最新的。
    store
        .end_attempt(&guarded.id, b1, T0 + 7, AttemptEnd::Reported)
        .unwrap();
    let b2 = store.begin_attempt(&guarded.id, T0 + 8).unwrap();
    store
        .record_report(
            &guarded.id,
            b2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第二次结果：5 行".to_owned(),
                question: None,
            },
            T0 + 9,
        )
        .unwrap();
    assert!(
        !store
            .effect_pending(EffectKind::Launch, gate.id.as_str())
            .unwrap(),
        "门在跑时不新排 launch"
    );
    let gate_now = store.task(&gate.id).unwrap().unwrap();
    assert_eq!(
        gate_now.needs[0].handoff.as_ref().unwrap().summary,
        "第二次结果：5 行"
    );
    // 门这一轮报回（核的是旧候选）：结束后只再放一次，候选是最新的。
    store
        .record_report(
            &gate.id,
            gate_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "核查了第一次结果".to_owned(),
                question: None,
            },
            T0 + 10,
        )
        .unwrap();
    store
        .end_attempt(&gate.id, gate_attempt, T0 + 10, AttemptEnd::Reported)
        .unwrap();
    assert!(
        store
            .effect_pending(EffectKind::Launch, gate.id.as_str())
            .unwrap(),
        "门结束后再放一次"
    );
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 11)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    assert!(
        !store
            .effect_pending(EffectKind::Launch, gate.id.as_str())
            .unwrap(),
        "只再放一次"
    );
    let gate_task = store.task(&gate.id).unwrap().unwrap();
    let b2_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let gate_attempt2 = store
        .begin_attempt_with(
            &gate.id,
            T0 + 12,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: b2_report.id.clone(),
                    requirement_version: b2_report.version,
                },
            },
            vec![Handoff {
                report_id: b2_report.id.clone(),
                requirement_version: b2_report.version,
                summary: b2_report.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            gate_attempt2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "核查了第二次结果".to_owned(),
                question: None,
            },
            T0 + 13,
        )
        .unwrap();
    let latest_gate_report = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let outcome = store
        .judge_gate(
            &gate.id,
            &latest_gate_report.id,
            GateConclusion::Pass,
            None,
            T0 + 14,
        )
        .unwrap();
    assert_eq!(outcome.released, vec!["再写结果".to_owned()]);
    assert!(
        store
            .effect_pending(EffectKind::Launch, downstream.id.as_str())
            .unwrap()
    );
    let _ = gate_task;
}

#[test]
fn upstream_new_ready_marks_downstream_reevaluation_and_relays() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    let root = store
        .create_graph(two_item_graph(cwd, None, None), T0)
        .unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    let children = store.children(&root.id).unwrap();
    let (upstream, downstream) = (children[0].clone(), children[1].clone());
    // A 第一次 ready：B 被放出并交接 A1。
    let a1 = store.begin_attempt(&upstream.id, T0 + 2).unwrap();
    store
        .record_report(
            &upstream.id,
            a1,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "A 第一次结果".to_owned(),
                question: None,
            },
            T0 + 3,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 4)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let a1_report = store
        .task(&upstream.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let b_attempt = store
        .begin_attempt_with(
            &downstream.id,
            T0 + 5,
            AttemptTrigger::Launch,
            vec![Handoff {
                report_id: a1_report.id.clone(),
                requirement_version: a1_report.version,
                summary: a1_report.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(
            &downstream.id,
            b_attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "B 第一次结果".to_owned(),
                question: None,
            },
            T0 + 6,
        )
        .unwrap();
    assert!(!store.needs_reevaluation(&downstream.id).unwrap());
    // A 第二次 ready：B 已有结果 → 需重评，并排一条 upstream_changed relay。
    store
        .end_attempt(&upstream.id, a1, T0 + 7, AttemptEnd::Reported)
        .unwrap();
    let a2 = store.begin_attempt(&upstream.id, T0 + 8).unwrap();
    store
        .record_report(
            &upstream.id,
            a2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "A 第二次结果：5 行".to_owned(),
                question: None,
            },
            T0 + 9,
        )
        .unwrap();
    assert!(
        store.needs_reevaluation(&downstream.id).unwrap(),
        "B 要按 A 的新结果重评"
    );
    // 队列里前面还有报告的 relay；领到 upstream_changed 为止，其它先收掉。
    let mut found = None;
    while let Some(claim) = store.claim_effect(EffectKind::Relay, T0 + 10).unwrap() {
        if matches!(claim.payload, EffectPayload::UpstreamChanged { .. }) {
            found = Some(claim);
            break;
        }
        store.complete_effect(&claim).unwrap();
    }
    let claim = found.expect("upstream_changed relay");
    let EffectPayload::UpstreamChanged {
        upstream_task,
        downstream_task,
        upstream_report,
    } = claim.payload.clone()
    else {
        panic!("relay 载荷：{:?}", claim.payload);
    };
    assert_eq!(upstream_task, upstream.id);
    assert_eq!(downstream_task, downstream.id);
    assert_eq!(
        upstream_report,
        store
            .task(&upstream.id)
            .unwrap()
            .unwrap()
            .latest_report()
            .unwrap()
            .id
    );
    // B 按新结果重新开跑（交接 A2）并报回：需重评清除。
    store
        .end_attempt(&downstream.id, b_attempt, T0 + 11, AttemptEnd::Reported)
        .unwrap();
    let handoffs = store
        .current_handoffs(&store.task(&downstream.id).unwrap().unwrap())
        .unwrap();
    let b2 = store
        .begin_attempt_with(&downstream.id, T0 + 12, AttemptTrigger::Launch, handoffs)
        .unwrap();
    store
        .record_report(
            &downstream.id,
            b2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "B 按新结果核对完了".to_owned(),
                question: None,
            },
            T0 + 13,
        )
        .unwrap();
    assert!(!store.needs_reevaluation(&downstream.id).unwrap());
}

#[test]
fn gate_pass_new_candidate_marks_guarded_downstream_reevaluation() {
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
    let claim = store.claim_effect(EffectKind::Launch, T0 + 2).unwrap();
    store.complete_effect(&claim.unwrap()).unwrap();
    // B 第一次 ready → 门开跑 → 判通过 → C 放出并交接 pass1。
    let b1 = store.begin_attempt(&guarded.id, T0 + 3).unwrap();
    store
        .record_report(
            &guarded.id,
            b1,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "B 第一版".to_owned(),
                question: None,
            },
            T0 + 4,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 5)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let b1_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let handoff1 = Handoff {
        report_id: b1_report.id.clone(),
        requirement_version: b1_report.version,
        summary: b1_report.summary.clone(),
    };
    let gate1 = store
        .begin_attempt_with(
            &gate.id,
            T0 + 6,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: b1_report.id.clone(),
                    requirement_version: b1_report.version,
                },
            },
            vec![handoff1.clone()],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            gate1,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第一版核查".to_owned(),
                question: None,
            },
            T0 + 7,
        )
        .unwrap();
    store
        .end_attempt(&gate.id, gate1, T0 + 7, AttemptEnd::Reported)
        .unwrap();
    let gate_report1 = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    store
        .judge_gate(
            &gate.id,
            &gate_report1.id,
            GateConclusion::Pass,
            None,
            T0 + 8,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 9)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let c1 = store
        .begin_attempt_with(
            &downstream.id,
            T0 + 10,
            AttemptTrigger::Launch,
            vec![handoff1],
        )
        .unwrap();
    store
        .record_report(
            &downstream.id,
            c1,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "C 第一版".to_owned(),
                question: None,
            },
            T0 + 11,
        )
        .unwrap();
    assert!(!store.needs_reevaluation(&downstream.id).unwrap());
    // B 第二版 → 门重跑 → 判通过：C 的已交结果要按新通过候选重评。
    store
        .end_attempt(&guarded.id, b1, T0 + 12, AttemptEnd::Reported)
        .unwrap();
    let b2 = store.begin_attempt(&guarded.id, T0 + 13).unwrap();
    store
        .record_report(
            &guarded.id,
            b2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "B 第二版：5 行".to_owned(),
                question: None,
            },
            T0 + 14,
        )
        .unwrap();
    let claim = store
        .claim_effect(EffectKind::Launch, T0 + 15)
        .unwrap()
        .unwrap();
    store.complete_effect(&claim).unwrap();
    let b2_report = store
        .task(&guarded.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    let gate2 = store
        .begin_attempt_with(
            &gate.id,
            T0 + 16,
            AttemptTrigger::GateRecheck {
                candidate: GateCandidate {
                    report_id: b2_report.id.clone(),
                    requirement_version: b2_report.version,
                },
            },
            vec![Handoff {
                report_id: b2_report.id.clone(),
                requirement_version: b2_report.version,
                summary: b2_report.summary.clone(),
            }],
        )
        .unwrap();
    store
        .record_report(
            &gate.id,
            gate2,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "第二版核查".to_owned(),
                question: None,
            },
            T0 + 17,
        )
        .unwrap();
    store
        .end_attempt(&gate.id, gate2, T0 + 17, AttemptEnd::Reported)
        .unwrap();
    let gate_report2 = store
        .task(&gate.id)
        .unwrap()
        .unwrap()
        .latest_report()
        .cloned()
        .unwrap();
    store
        .judge_gate(
            &gate.id,
            &gate_report2.id,
            GateConclusion::Pass,
            None,
            T0 + 18,
        )
        .unwrap();
    assert!(
        store.needs_reevaluation(&downstream.id).unwrap(),
        "C 要按 B 的新通过候选重评"
    );
    // 队列前面还有别的 relay（含 gate verdict）；领到 upstream_changed 为止。
    let mut found = false;
    while let Some(claim) = store.claim_effect(EffectKind::Relay, T0 + 19).unwrap() {
        if matches!(claim.payload, EffectPayload::UpstreamChanged { .. }) {
            found = true;
            break;
        }
        store.complete_effect(&claim).unwrap();
    }
    assert!(found, "要有 upstream_changed relay");
}

#[test]
fn cancelled_graph_gets_no_launch_and_stays_out_of_pending() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    let root = store.create_graph(gated_graph(cwd), T0).unwrap();
    let cancellation = store
        .cancel_graph(&root.id, CancellationReason::Dismissed, T0 + 1)
        .unwrap();
    assert_eq!(cancellation.reason, CancellationReason::Dismissed);
    assert_eq!(cancellation.at, T0 + 1);
    // 同一个取消重复点：返回同一条，不改时间。
    let again = store
        .cancel_graph(&root.id, CancellationReason::NoCard, T0 + 2)
        .unwrap();
    assert_eq!(again, cancellation);
    // 再确认已经变了。
    assert!(matches!(
        store.confirm_graph(&root.id, T0 + 3),
        Err(TaskError::GraphCancelled)
    ));
    // 子任务不排队、随时是待补足条件；没有任何 launch。
    for child in store.children(&root.id).unwrap() {
        assert_eq!(store.stage(&child.id).unwrap(), Stage::NeedsConditions);
        assert!(
            !store
                .effect_pending(EffectKind::Launch, child.id.as_str())
                .unwrap()
        );
    }
    assert_eq!(store.stage(&root.id).unwrap(), Stage::NeedsConditions);
}

#[test]
fn confirmed_graph_cannot_be_cancelled() {
    let directory = tempfile::tempdir().unwrap();
    let cwd = directory.path();
    let store = open_store().1;
    let root = store
        .create_graph(two_item_graph(cwd, None, None), T0)
        .unwrap();
    store.confirm_graph(&root.id, T0 + 1).unwrap();
    assert!(matches!(
        store.cancel_graph(&root.id, CancellationReason::Replaced, T0 + 2),
        Err(TaskError::GraphAlreadyConfirmed)
    ));
}
