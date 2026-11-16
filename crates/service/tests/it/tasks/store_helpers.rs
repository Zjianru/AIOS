//! 任务库读取辅助：取任务、attempt 状态、上报状态、重载。

use super::*;

pub(super) fn tasks_store(data: &Path) -> Store {
    Store::open(data.join(service::TASKS_DB_FILE)).expect("任务库")
}

pub(super) fn one_task(store: &Store) -> Task {
    let ids = store.task_ids().expect("任务 id");
    assert_eq!(ids.len(), 1, "只应有一个任务：{ids:?}");
    store.task(&ids[0]).expect("读任务").expect("任务存在")
}

/// 一个 Attempt 以什么理由结束；还没结束或没有 Attempt 时是 None。
pub(super) fn attempt_end(store: &Store, task_id: &tasks::TaskId) -> Option<tasks::AttemptEnd> {
    store
        .task(task_id)
        .ok()
        .flatten()
        .and_then(|task| task.latest_attempt().and_then(|attempt| attempt.end))
}

/// 任务最近一份报告是不是 ready。
pub(super) fn reported_ready(store: &Store, task_id: &tasks::TaskId) -> bool {
    store
        .task(task_id)
        .ok()
        .flatten()
        .and_then(|task| task.latest_report().map(|report| report.state))
        == Some(ReportState::Ready)
}

/// 任务当前 Attempt 还开着，而且已经有任务会话。
pub(super) fn attempt_in_flight(store: &Store, task_id: &tasks::TaskId) -> bool {
    let Ok(Some(task)) = store.task(task_id) else {
        return false;
    };
    task.latest_attempt()
        .is_some_and(|attempt| attempt.ended_at.is_none() && attempt.conversation.is_some())
}

pub(super) fn attempt_count(task: &Task) -> usize {
    task.runs.iter().map(|run| run.attempts.len()).sum()
}

/// 重新读当前的任务聚合（启动扫描收尾之后）。
pub(super) fn reload_tasks(store: &Store, tasks: &[Task]) -> Vec<Task> {
    tasks
        .iter()
        .map(|task| store.task(&task.id).expect("读任务").expect("任务存在"))
        .collect()
}
