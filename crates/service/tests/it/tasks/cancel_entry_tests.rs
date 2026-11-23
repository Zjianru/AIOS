//! T077.5：真 RootHost/TaskRuntime 上的 cancel_tasks act：active 被真实取消，
//! 已关闭/已取消逐项拒绝，summary 和 data 都是逐项回执。

use super::*;

fn draft(title: &str) -> NewTask {
    NewTask {
        title: title.to_owned(),
        requirement: "做".to_owned(),
        dod: "做完".to_owned(),
        cwd: std::env::temp_dir(),
        conversation: ConversationId::new("kind-cancel-entry"),
        model: ModelChoice {
            connection: "deepseek".to_owned(),
            model: "deepseek-flash".to_owned(),
            effort_wanted: "high".to_owned(),
        },
        tools: vec!["read".to_owned()],
    }
}

fn close(store: &Store, task: &Task) {
    let attempt = store.begin_attempt(&task.id, 1_700_000_000_001).unwrap();
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
            1_700_000_000_002,
        )
        .unwrap();
    store.accept(&task.id, &report, 1_700_000_000_003).unwrap();
}

#[tokio::test]
async fn cancel_tasks_act_mixes_active_closed_and_already_cancelled() {
    let fixture = Fixture::new_with_actions().await;
    let store = super::store_helpers::tasks_store(fixture.data());
    let active = store
        .create_task(draft("可取消"), 1_700_000_000_000)
        .unwrap();
    let closed = store
        .create_task(draft("已完成"), 1_700_000_000_000)
        .unwrap();
    close(&store, &closed);
    let gone = store
        .create_task(draft("已取消"), 1_700_000_000_000)
        .unwrap();
    store
        .cancel(
            &gone.id,
            tasks::CancellationSource::User,
            None,
            1_700_000_000_010,
        )
        .unwrap();

    let response = fixture
        .host
        .dispatch(service::Call::ActionAct(service::ActionActRequest {
            action: "cancel_tasks".to_owned(),
            details: serde_json::json!({
                "task_ids": [active.id.as_str(), closed.id.as_str(), gone.id.as_str()]
            }),
        }))
        .await
        .expect("action-act");
    let service::Response::ActionAct(outcome) = response else {
        panic!("不是 action-act 响应");
    };
    assert_eq!(
        outcome.data["cancelled"],
        serde_json::json!([active.id.as_str()])
    );
    let refused = outcome.data["refused"].as_array().unwrap();
    assert_eq!(refused.len(), 2);
    assert_eq!(refused[0]["task_id"], closed.id.as_str());
    assert_eq!(refused[0]["reason"], "已关闭");
    assert_eq!(refused[1]["task_id"], gone.id.as_str());
    assert_eq!(refused[1]["reason"], "已取消");
    assert!(outcome.summary.contains("取消了 1 项"));
    assert!(outcome.summary.contains("没取消 2 项"));
    let stored = store.task(&active.id).unwrap().unwrap();
    assert!(stored.cancelled.is_some(), "active 任务要真被取消");
    assert_eq!(store.stage(&active.id).unwrap(), Stage::Closed);
}

#[tokio::test]
async fn cancel_tasks_act_reports_actual_downstream_ids() {
    let fixture = Fixture::new_with_actions().await;
    let store = super::store_helpers::tasks_store(fixture.data());
    let cwd = std::env::temp_dir();
    fn subtask(cwd: &std::path::Path, title: &str, after: Vec<usize>) -> tasks::NewSubtask {
        tasks::NewSubtask {
            task: NewTask {
                title: title.to_owned(),
                requirement: "做".to_owned(),
                dod: "做完".to_owned(),
                cwd: cwd.to_path_buf(),
                conversation: ConversationId::new("kind-cancel-downstream"),
                model: ModelChoice {
                    connection: "deepseek".to_owned(),
                    model: "deepseek-flash".to_owned(),
                    effort_wanted: "high".to_owned(),
                },
                tools: vec!["read".to_owned()],
            },
            policy: tasks::TaskPolicy {
                write_scope: None,
                worktree: false,
                bash: false,
            },
            after,
            milestone: false,
            program_gate: Vec::new(),
        }
    }
    let graph = store
        .create_graph(
            tasks::NewGraph {
                root: draft("总任务"),
                subtasks: vec![subtask(&cwd, "甲", vec![]), subtask(&cwd, "乙", vec![0])],
            },
            1_700_000_000_000,
        )
        .unwrap();
    store.confirm_graph(&graph.id, 1_700_000_000_001).unwrap();
    let children = store.children(&graph.id).unwrap();
    let first = children
        .iter()
        .find(|task| task.title == "甲")
        .unwrap()
        .clone();
    let second = children
        .iter()
        .find(|task| task.title == "乙")
        .unwrap()
        .clone();

    let response = fixture
        .host
        .dispatch(service::Call::ActionAct(service::ActionActRequest {
            action: "cancel_tasks".to_owned(),
            details: serde_json::json!({ "task_ids": [first.id.as_str()] }),
        }))
        .await
        .expect("action-act");
    let service::Response::ActionAct(outcome) = response else {
        panic!("不是 action-act 响应");
    };
    let cancelled: Vec<&str> = outcome.data["cancelled"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert!(cancelled.contains(&first.id.as_str()));
    assert!(cancelled.contains(&second.id.as_str()));
    assert!(outcome.summary.contains("取消 2 项"));
}
