//! 后台任务结算能力（T006.1e）：把结算回调与完成通知投递从 loop 移出。
//!
//! 注册表仍在 tools/jobs.rs；这里只负责：结算线程上发 JobCompleted，回到 runtime
//! 上下文把完成通知写成 durable inbox 输入（T006.2a），再唤醒驱动。

use std::sync::Arc;

use crate::capabilities::LoopAccess;
use crate::driver::Inner;
use crate::{AgentError, AgentEvent, JobStatus};

/// 结算回调安装（Agent::start 调用一次；用 Weak 避免循环引用）。
pub(crate) fn install(inner: &Arc<Inner>) {
    let handle = tokio::runtime::Handle::current();
    let weak = Arc::downgrade(inner);
    inner.job_session().tasks().set_on_settled(Arc::new(
        move |job_id: &str, status: tools::JobStatus| {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            let job_id = job_id.to_owned();
            let handle = handle.clone();
            handle.spawn(async move {
                if let Err(error) = deliver(inner.as_ref(), &job_id, status).await {
                    // 结算线程是后台边界：没有调用方接错误；与 Status 派发一致，
                    // 写日志失败不打断唤醒（T006.2a known：通知丢失，进程继续）。
                    let _ = error;
                }
                inner.wake();
            });
        },
    ));
}

/// 格式化完成通知、发 JobCompleted、把通知写成 durable 的下一步输入。
async fn deliver(
    host: &dyn LoopAccess,
    job_id: &str,
    status: tools::JobStatus,
) -> Result<(), AgentError> {
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
    host.enqueue_context(text).await
}
