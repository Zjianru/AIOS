//! 一次 step 的工具批执行：独占调用是屏障，并行调用进有界滚动池，结果按模型顺序。
//!
//! 机制照 DSH 的 packages/core/tools/src/batch.ts（strategy 第 1 节）。
//! 取消语义（T004.3）：观察到取消后不再派发新调用，等已开始的调用结束并按模型顺序
//! 提交结果，未派发的调用补 `ABORTED_BEFORE_DISPATCH`。

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolResult, ToolSet};

/// 一次批执行的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchOutcome {
    /// 按模型顺序的结果；取消时未派发的调用带 `ABORTED_BEFORE_DISPATCH`。
    pub results: Vec<ToolResult>,
    /// 是否观察到取消；包括所有调用都已派发/结束后才看到取消的情况（此时可能没有
    /// `ABORTED_BEFORE_DISPATCH` 结果）。
    pub cancelled: bool,
}

/// 未派发调用的合成结果（文案和错误码照 DSH 的 batch.ts skip()）。
fn aborted_result(call: &ToolCall) -> ToolResult {
    ToolResult {
        call_id: call.id.clone(),
        content: "Error: tool call aborted before dispatch".to_owned(),
        is_error: true,
        error_code: Some("ABORTED_BEFORE_DISPATCH".to_owned()),
    }
}

/// 一个待执行的调用（已按工具分类）。
struct Planned<'a> {
    call: &'a ToolCall,
    tool: Option<&'a dyn Tool>,
}

/// 并行池里的一个 future：调用下标 + 未完成的执行。
type PendingFuture<'a, T> = (usize, Pin<Box<dyn Future<Output = T> + Send + 'a>>);

async fn invoke<'a>(plan: &'a Planned<'_>, context: &'a ToolContext) -> ToolResult {
    match plan.tool {
        Some(tool) => {
            // T005.9：工具自己声明了单次调用时间上限时，超时返回 TOOL_TIMEOUT 错误结果
            // （照 DSH timeout-policy）。超时的是这一次工具调用，不影响本轮后续请求。
            let Some(timeout_ms) = tool.timeout_ms() else {
                return tool.execute(plan.call.clone(), context).await;
            };
            match tokio::time::timeout(
                std::time::Duration::from_millis(timeout_ms),
                tool.execute(plan.call.clone(), context),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => ToolResult {
                    call_id: plan.call.id.clone(),
                    content: format!("Error: tool call timed out after {timeout_ms}ms"),
                    is_error: true,
                    error_code: Some("TOOL_TIMEOUT".to_owned()),
                },
            }
        }
        None => ToolResult::error(
            plan.call.id.clone(),
            crate::ToolFailure::coded(
                "TOOL_NOT_FOUND",
                format!("tool \"{}\" is not available", plan.call.name),
            ),
        ),
    }
}

/// 取下一个已完成的 future；返回它在列表里的位置（用于按下标收回结果）。
async fn next_finished<'a, T>(futures: &mut Vec<PendingFuture<'a, T>>) -> (usize, T) {
    std::future::poll_fn(|cx| {
        for (position, (_, future)) in futures.iter_mut().enumerate() {
            if let Poll::Ready(value) = future.as_mut().poll(cx) {
                return Poll::Ready((position, value));
            }
        }
        Poll::Pending
    })
    .await
}

/// 跑一段连续的并行安全调用：上限内滚动补位；观察到取消后不再补位，等已开始的结束。
async fn run_parallel<'a>(
    plans: &'a [Planned<'_>],
    max_parallel: usize,
    context: &'a ToolContext,
    cancel: &AtomicBool,
) -> (Vec<Option<ToolResult>>, bool) {
    let mut results: Vec<Option<ToolResult>> = plans.iter().map(|_| None).collect();
    let mut futures: Vec<PendingFuture<'a, ToolResult>> = Vec::new();
    let mut next = 0;
    let mut cancelled = false;
    while next < plans.len() && futures.len() < max_parallel && !cancel.load(Ordering::SeqCst) {
        futures.push((next, Box::pin(invoke(&plans[next], context))));
        next += 1;
    }
    while !futures.is_empty() {
        let (position, result) = next_finished(&mut futures).await;
        let (index, _) = futures.remove(position);
        results[index] = Some(result);
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
        } else if next < plans.len() {
            futures.push((next, Box::pin(invoke(&plans[next], context))));
            next += 1;
        }
    }
    if next < plans.len() {
        cancelled = true;
    }
    (results, cancelled)
}

/// 执行一次 step 的全部工具调用，结果按模型给出的顺序返回。
///
/// 独占调用单独成立并形成屏障；连续的并行安全调用进上限为 max_parallel 的
/// 滚动池。模型顺序即结果顺序，与完成顺序无关。取消信号置位后：已开始的调用
/// 等结束并保留结果，未开始的补 `ABORTED_BEFORE_DISPATCH`。
pub async fn execute_batch(
    tools: &ToolSet,
    calls: &[ToolCall],
    max_parallel: usize,
    context: &ToolContext,
    cancel: &AtomicBool,
) -> BatchOutcome {
    assert!(max_parallel >= 1, "max_parallel_tool_calls 必须是正整数");
    let plans: Vec<Planned<'_>> = calls
        .iter()
        .map(|call| Planned {
            call,
            tool: tools.get(&call.name),
        })
        .collect();
    let mut results: Vec<Option<ToolResult>> = plans.iter().map(|_| None).collect();
    let mut index = 0;
    let mut cancelled = false;
    while index < plans.len() {
        if cancel.load(Ordering::SeqCst) {
            cancelled = true;
            break;
        }
        let mode = plans[index]
            .tool
            .map_or(ToolConcurrency::Exclusive, Tool::concurrency);
        match mode {
            ToolConcurrency::Exclusive => {
                results[index] = Some(invoke(&plans[index], context).await);
                index += 1;
            }
            ToolConcurrency::Parallel => {
                let mut end = index;
                while end < plans.len()
                    && plans[end]
                        .tool
                        .is_some_and(|tool| tool.concurrency() == ToolConcurrency::Parallel)
                {
                    end += 1;
                }
                let (group_results, group_cancelled) =
                    run_parallel(&plans[index..end], max_parallel, context, cancel).await;
                for (offset, result) in group_results.into_iter().enumerate() {
                    results[index + offset] = result;
                }
                cancelled |= group_cancelled;
                index = end;
            }
        }
    }
    if results.iter().any(Option::is_none) {
        cancelled = true;
    }
    let results = plans
        .iter()
        .enumerate()
        .map(|(position, plan)| {
            results[position]
                .take()
                .unwrap_or_else(|| aborted_result(plan.call))
        })
        .collect();
    BatchOutcome { results, cancelled }
}
