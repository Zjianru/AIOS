//! T077.3：取消单项事实、阶段、事务效果和写入口守卫。

use std::path::PathBuf;

use super::*;

const T0: i64 = 1_700_000_000_000;

fn open() -> (tempfile::TempDir, Store) {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    (directory, store)
}

fn draft() -> NewTask {
    NewTask {
        title: "查一下取消".to_owned(),
        requirement: "读文档".to_owned(),
        dod: "给出例子".to_owned(),
        cwd: PathBuf::from("/tmp/cancel-cwd"),
        conversation: ConversationId::new("kind-session"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-chat".to_owned(),
            effort_wanted: "default".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn task(store: &Store) -> Task {
    let task = store.create_task(draft(), T0).unwrap();
    // 清掉建任务排的 launch，后面好断言取消效果。
    let _ = store.claim_effect(EffectKind::Launch, T0).unwrap();
    task
}

#[test]
fn cancel_is_the_first_stage_fact_and_enqueues_cancel_effect_in_one_transaction() {
    let (_directory, store) = open();
    let task = task(&store);
    let cancelled = store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    assert_eq!(cancelled.at, T0 + 10);
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.cancelled, Some(cancelled));
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    assert_eq!(store.closed_at(&task.id).unwrap(), Some(T0 + 10));
    let claim = store.claim_effect(EffectKind::Cancel, T0 + 10).unwrap();
    assert_eq!(
        claim.map(|claim| claim.payload),
        Some(EffectPayload::Cancel {
            task_id: task.id.clone()
        })
    );
}

#[test]
fn cancelled_task_rejects_every_write_entry_with_the_same_sentence() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .end_attempt(&task.id, attempt, T0 + 2, AttemptEnd::Interrupted)
        .unwrap();
    store
        .cancel(&task.id, CancellationSource::Kind, None, T0 + 10)
        .unwrap();
    let message = CANCELLED_MESSAGE;
    assert_eq!(
        store
            .begin_attempt(&task.id, T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        store
            .say(&task.id, "再说一句", T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        store
            .update_requirement(&task.id, "新要求", None, T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    assert_eq!(
        store
            .update_write_scope(&task.id, "notes", T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
    let report = NewReport {
        state: ReportState::Ready,
        origin: ReportOrigin::Agent,
        summary: "做完了".to_owned(),
        question: None,
    };
    let _ = report;
    assert_eq!(
        store
            .accept(&task.id, &ReportId::from("missing"), T0 + 20)
            .unwrap_err()
            .to_string(),
        message
    );
}

#[test]
fn late_report_is_kept_in_history_but_does_not_change_cancelled_stage() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "取消后迟到报告".to_owned(),
                question: None,
            },
            T0 + 20,
        )
        .unwrap();
    let stored = store.task(&task.id).unwrap().unwrap();
    assert_eq!(stored.reports.len(), 1);
    assert_eq!(stored.latest_report().unwrap().summary, "取消后迟到报告");
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
}

#[test]
fn group_accept_refuses_a_cancelled_task_without_accepting_others_silently() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    let report = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "先交结果".to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
    store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    let batch = store
        .accept_many(&[(task.id.clone(), report)], T0 + 20)
        .unwrap();
    assert_eq!(batch.outcomes.len(), 1);
    assert_eq!(
        batch.outcomes[0].refused.as_deref(),
        Some(CANCELLED_MESSAGE)
    );
}

fn graph_draft(cwd: &std::path::Path, title: &str) -> NewTask {
    let mut task = draft();
    task.title = title.to_owned();
    task.requirement = format!("{title} 的要求");
    task.dod = format!("{title} 的 DoD");
    task.cwd = cwd.to_path_buf();
    task
}

fn subtask(cwd: &std::path::Path, title: &str, after: Vec<usize>, gate: bool) -> NewSubtask {
    NewSubtask {
        task: graph_draft(cwd, title),
        policy: TaskPolicy {
            write_scope: (!gate).then(|| "out.txt".to_owned()),
            worktree: false,
            bash: false,
        },
        after,
        milestone: gate,
        program_gate: Vec::new(),
    }
}

#[test]
fn cascade_cancels_downstream_recursively_and_their_gates_but_not_the_root_fact() {
    let directory = tempfile::tempdir().unwrap();
    let store = Store::open(directory.path().join("tasks.sqlite")).unwrap();
    let cwd = PathBuf::from("/tmp/cascade-cwd");
    let graph = NewGraph {
        root: graph_draft(&cwd, "总任务"),
        subtasks: vec![
            subtask(&cwd, "甲", vec![], false),
            subtask(&cwd, "乙", vec![0], false),
            subtask(&cwd, "丙", vec![1], false),
            subtask(&cwd, "甲的门", vec![0], true),
        ],
    };
    let root = store.create_graph(graph, T0).unwrap();
    let children = store.children(&root.id).unwrap();
    let by_title = |title: &str| {
        children
            .iter()
            .find(|task| task.title == title)
            .unwrap()
            .clone()
    };
    let first = by_title("甲");
    let second = by_title("乙");
    let third = by_title("丙");
    let gate = by_title("甲的门");
    // 下游乙正在跑：取消要交给效果泵去立即停。
    let attempt = store.begin_attempt(&second.id, T0 + 1).unwrap();

    let cancelled = store
        .cancel_with_downstream(&first.id, CancellationSource::User, None, T0 + 10)
        .unwrap();
    assert_eq!(cancelled.len(), 4, "目标+乙+丙+门");
    for task in [&first, &second, &third, &gate] {
        let stored = store.task(&task.id).unwrap().unwrap();
        assert!(stored.cancelled.is_some(), "{} 应被连带取消", task.title);
        assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    }
    assert_eq!(
        store
            .task(&second.id)
            .unwrap()
            .unwrap()
            .cancelled
            .unwrap()
            .from,
        Some(first.id.clone())
    );
    assert_eq!(
        store
            .task(&gate.id)
            .unwrap()
            .unwrap()
            .cancelled
            .unwrap()
            .from,
        Some(first.id.clone())
    );
    assert_eq!(store.task(&root.id).unwrap().unwrap().cancelled, None);
    assert_eq!(
        store
            .task(&second.id)
            .unwrap()
            .unwrap()
            .latest_attempt()
            .unwrap()
            .id,
        attempt,
        "在跑下游的 Attempt 行还在，停止由效果泵做"
    );
    for task in [&first, &second, &third, &gate] {
        assert!(
            store
                .effect_pending(EffectKind::Cancel, task.id.as_str())
                .unwrap(),
            "{} 要排 cancel 效果",
            task.title
        );
    }
    let claimed = store.claim_effect(EffectKind::Cancel, T0 + 20).unwrap();
    assert!(matches!(
        claimed.map(|claim| claim.payload),
        Some(EffectPayload::Cancel { .. })
    ));
}

#[test]
fn closed_task_cannot_be_cancelled_and_writes_nothing() {
    let (_directory, store) = open();
    let task = task(&store);
    let attempt = store.begin_attempt(&task.id, T0 + 1).unwrap();
    let report = store
        .record_report(
            &task.id,
            attempt,
            NewReport {
                state: ReportState::Ready,
                origin: ReportOrigin::Agent,
                summary: "已完成".to_owned(),
                question: None,
            },
            T0 + 2,
        )
        .unwrap();
    store.accept(&task.id, &report, T0 + 3).unwrap();
    assert_eq!(store.stage(&task.id).unwrap(), Stage::Closed);
    let before = store.closed_at(&task.id).unwrap();
    let error = store
        .cancel(&task.id, CancellationSource::User, None, T0 + 10)
        .unwrap_err();
    assert!(matches!(error, TaskError::AlreadyClosed));
    let after = store.task(&task.id).unwrap().unwrap();
    assert_eq!(after.cancelled, None);
    assert_eq!(store.closed_at(&task.id).unwrap(), before);
    assert!(
        !store
            .effect_pending(EffectKind::Cancel, task.id.as_str())
            .unwrap(),
        "拒绝时不能排停效果"
    );
}
