//! T052：任务图的建立、确认、阶段汇总与总任务不跑执行。

use std::path::Path;

use tempfile::TempDir;

use crate::{
    ConversationId, EffectKind, EffectPayload, ModelChoice, NewGraph, NewReport, NewSubtask,
    NewTask, ReportOrigin, ReportState, Stage, Store, Task, TaskError, TaskPolicy,
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
