//! 后台任务生命周期能力（T006.1e，T006.9 补事件与 DSH 通知）：把注册/结算/完成通知从 loop 移出。
//!
//! 注册表仍在 tools/jobs.rs；这里只负责三件事（照 DSH jobs-local 的 recordSessionEvent +
//! tool-jobs 的完成通知）：
//! 1. 注册时写 `job/registered`、结算时写 `job/settled` 会话事件——每次注册/结算都写，
//!    和调用方有没有 await、通知发没发无关；
//! 2. 结算没有被调用方收走时，投递一条模型可见的完成通知（下一步输入）并唤醒驱动；
//! 3. 三种回调都送进同一条有序通道消费：注册 → 结算 → 通知，会话事件不会乱序。

use std::sync::Arc;

use session::{EventKind, JobToolCallData};
use tokio::sync::mpsc;

use crate::capabilities::LoopAccess;
use crate::driver::Inner;
use crate::{AgentError, AgentEvent, JobStatus};

/// DSH tool-jobs `inlineOutputBytes` 默认值：完成通知内联输出的字节上限。
const INLINE_OUTPUT_BYTES: usize = 2048;

/// 注册表回调送来的一条生命周期消息；单消费者按发生顺序写日志/投递通知。
enum Lifecycle {
    Registered(tools::JobRegistered),
    Settled(tools::JobSettled),
    Notify(tools::JobSettled),
}

/// 安装三个回调（Agent::start 调用一次；用 Weak 避免循环引用）。
pub(crate) fn install(inner: &Arc<Inner>) {
    let weak = Arc::downgrade(inner);
    let (sender, mut receiver) = mpsc::unbounded_channel::<Lifecycle>();
    let tasks = inner.job_session().tasks();
    let registered_sender = sender.clone();
    tasks.set_on_registered(Arc::new(move |registered: &tools::JobRegistered| {
        let _ = registered_sender.send(Lifecycle::Registered(registered.clone()));
    }));
    let settled_sender = sender.clone();
    tasks.set_on_settled(Arc::new(move |settled: &tools::JobSettled| {
        let _ = settled_sender.send(Lifecycle::Settled(settled.clone()));
    }));
    tasks.set_on_notify(Arc::new(move |settled: &tools::JobSettled| {
        let _ = sender.send(Lifecycle::Notify(settled.clone()));
    }));
    tokio::spawn(async move {
        while let Some(message) = receiver.recv().await {
            let Some(inner) = weak.upgrade() else {
                break;
            };
            match message {
                Lifecycle::Registered(registered) => {
                    let _ = inner.append(&[registered_event(&registered)]).await;
                }
                Lifecycle::Settled(settled) => {
                    let _ = inner.append(&[settled_event(&settled)]).await;
                }
                Lifecycle::Notify(settled) => {
                    if let Err(error) = deliver(inner.as_ref(), &settled).await {
                        // 后台边界：没有调用方接错误；写日志失败不打断唤醒（T006.2a known：通知丢失，进程继续）。
                        let _ = error;
                    }
                    inner.wake();
                }
            }
        }
    });
}

fn tool_call_data(tool_call: &Option<tools::JobToolCall>) -> Option<JobToolCallData> {
    tool_call.as_ref().map(|tool_call| JobToolCallData {
        call_id: tool_call.call_id.clone(),
        root_call_id: tool_call.root_call_id.clone(),
    })
}

/// `job/registered` 会话事件（DSH JobRegisteredData 字段）。
fn registered_event(registered: &tools::JobRegistered) -> EventKind {
    EventKind::JobRegistered {
        job_id: registered.id.clone(),
        owner: registered.owner.clone(),
        kind: registered.kind.to_owned(),
        label: registered.label.clone(),
        started_at: registered.started_at,
        tool_call: tool_call_data(&registered.tool_call),
    }
}

/// `job/settled` 会话事件（DSH JobSettledData = 注册字段 + 结算字段）。
fn settled_event(settled: &tools::JobSettled) -> EventKind {
    EventKind::JobSettled {
        job_id: settled.id.clone(),
        owner: settled.owner.clone(),
        kind: settled.kind.to_owned(),
        label: settled.label.clone(),
        started_at: settled.started_at,
        tool_call: tool_call_data(&settled.tool_call),
        finished_at: settled.finished_at,
        status: settled.status.as_str().to_owned(),
        cause: settled.cause.as_str().to_owned(),
        producer_reported: settled.producer_reported,
        detail: settled.detail.clone(),
    }
}

/// 状态行照 DSH tool-jobs render.ts 的 statusLine：有 detail 时 `[status: x, detail]`。
fn status_line(settled: &tools::JobSettled) -> String {
    match &settled.detail {
        Some(detail) => format!("[status: {}, {}]", settled.status.as_str(), detail),
        None => format!("[status: {}]", settled.status.as_str()),
    }
}

/// 照 DSH tool-jobs 的完成通知：`background job <id> (kind: label) finished [status: …]`，
/// 内联装得下输出就带 `. Output:\n…`（空输出 `. No new output.`），装不下就
/// `. Read its output with job_output.`。
///
/// 与 DSH 的一处适配：DSH 先取一次 consuming read 再决定装不装得下（装不下时把这次读暂存在
/// pending 里等 job_output 取）；AIOS 用 peek 先看，装不下就不推进游标，数据仍然留给 job_output。
async fn deliver(host: &dyn LoopAccess, settled: &tools::JobSettled) -> Result<(), AgentError> {
    let tasks = host.tool_session().tasks();
    let head = format!(
        "background job {} ({}: {}) finished {}",
        settled.id,
        settled.kind,
        settled.label,
        status_line(settled)
    );
    let inline = tasks.peek_output(&settled.id).ok().map(|read| {
        let body = tools::render_model_delta(&read);
        if body.is_empty() {
            format!("{head}. No new output.")
        } else {
            let body = body.strip_suffix('\n').unwrap_or(&body);
            format!("{head}. Output:\n{body}")
        }
    });
    let text = match inline.filter(|text| text.len() <= INLINE_OUTPUT_BYTES) {
        Some(text) => {
            // 通知已经内联这段输出：推进模型游标，后续 job_output 不再重复返回（DSH held read）。
            let _ = tasks.read_output(&settled.id);
            text
        }
        None => format!("{head}. Read its output with job_output."),
    };
    let agent_status = match settled.status {
        tools::JobStatus::Completed => JobStatus::Completed,
        tools::JobStatus::Failed => JobStatus::Failed,
        tools::JobStatus::Killed => JobStatus::Killed,
        tools::JobStatus::Running => JobStatus::Killed,
    };
    let _ = host.events().send(AgentEvent::JobCompleted {
        job_id: settled.id.clone(),
        status: agent_status,
        detail: Some(settled.label.clone()),
    });
    host.enqueue_context(text).await
}
