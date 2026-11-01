//! 后台任务结算能力（T006.1e）：把结算回调与完成通知投递从 loop 移出。
//!
//! 注册表仍在 tools/jobs.rs；这里只负责：结算线程上格式化通知 -> 投下一步/下一轮输入，
//! 发 AgentEvent::JobCompleted，并回到 runtime 上下文唤醒驱动。

use std::sync::Arc;

use crate::capabilities::LoopAccess;
use crate::driver::Inner;
use crate::{AgentEvent, JobStatus};

/// 结算回调安装（Agent::start 调用一次；用 Weak 避免循环引用）。
pub(crate) fn install(inner: &Arc<Inner>) {
    let handle = tokio::runtime::Handle::current();
    let weak = Arc::downgrade(inner);
    inner.job_session().tasks().set_on_settled(Arc::new(
        move |job_id: &str, status: tools::JobStatus| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            // 结算线程上同步决定目标队列（忙碌/空闲按结算瞬间的 phase），
            // 只有 wake 需要回到 runtime 上下文。
            deliver(inner.as_ref(), job_id, status);
            handle.spawn(async move {
                inner.wake();
            });
        },
    ));
}

fn deliver(host: &dyn LoopAccess, job_id: &str, status: tools::JobStatus) {
    let read = host.tool_session().tasks().output(job_id).ok();
    let label = read
        .as_ref()
        .map_or_else(String::new, |read| read.view.label.clone());
    let mut text = format!(
        "background job {job_id} (bash: {label}) finished [status: {}].",
        status.as_str()
    );
    if let Some(read) = &read {
        if read.stdout.len() <= 2048 && read.stderr.is_empty() {
            text.push_str("\nOutput:\n");
            text.push_str(&read.stdout);
        } else {
            text.push_str("\nRead its output with job_output.");
        }
    }
    let agent_status = match status {
        tools::JobStatus::Completed => JobStatus::Completed,
        tools::JobStatus::Failed => JobStatus::Failed,
        tools::JobStatus::Killed | tools::JobStatus::Running => JobStatus::Killed,
    };
    let _ = host.events().send(AgentEvent::JobCompleted {
        job_id: job_id.to_owned(),
        status: agent_status,
        detail: Some(label),
    });
    host.enqueue_context(text);
}
