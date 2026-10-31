//! 一次 step 的工具批执行：独占调用是屏障，并行调用进有界滚动池，结果按模型顺序。
//!
//! 机制照 DSH 的 packages/core/tools/src/batch.ts（strategy 第 1 节）。
//! T004.2 先不带取消路径（取消在 T004.3 补）。

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use crate::{Tool, ToolCall, ToolConcurrency, ToolContext, ToolResult, ToolSet};

/// 一个待执行的调用（已按工具分类）。
struct Planned<'a> {
    call: &'a ToolCall,
    tool: Option<&'a dyn Tool>,
}

/// 并行池里的一个 future：调用下标 + 未完成的执行。
type PendingFuture<'a, T> = (usize, Pin<Box<dyn Future<Output = T> + Send + 'a>>);

async fn invoke<'a>(plan: &'a Planned<'_>, context: &'a ToolContext) -> ToolResult {
    match plan.tool {
        Some(tool) => tool.execute(plan.call.clone(), context).await,
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

async fn run_parallel<'a>(
    plans: &'a [Planned<'_>],
    max_parallel: usize,
    context: &'a ToolContext,
) -> Vec<ToolResult> {
    let mut results: Vec<Option<ToolResult>> = plans.iter().map(|_| None).collect();
    let mut futures: Vec<PendingFuture<'a, ToolResult>> = Vec::new();
    let mut next = 0;
    while next < plans.len() && futures.len() < max_parallel {
        futures.push((next, Box::pin(invoke(&plans[next], context))));
        next += 1;
    }
    while !futures.is_empty() {
        let (position, result) = next_finished(&mut futures).await;
        let (index, _) = futures.remove(position);
        results[index] = Some(result);
        if next < plans.len() {
            futures.push((next, Box::pin(invoke(&plans[next], context))));
            next += 1;
        }
    }
    results
        .into_iter()
        .map(|result| result.expect("并行池没有漏收结果"))
        .collect()
}

/// 执行一次 step 的全部工具调用，结果按模型给出的顺序返回。
///
/// 独占调用单独成立并形成屏障；连续的并行安全调用进上限为 max_parallel 的
/// 滚动池。模型顺序即结果顺序，与完成顺序无关。
pub async fn execute_batch(
    tools: &ToolSet,
    calls: &[ToolCall],
    max_parallel: usize,
    context: &ToolContext,
) -> Vec<ToolResult> {
    assert!(max_parallel >= 1, "max_parallel_tool_calls 必须是正整数");
    let plans: Vec<Planned<'_>> = calls
        .iter()
        .map(|call| Planned {
            call,
            tool: tools.get(&call.name),
        })
        .collect();
    let mut results = Vec::with_capacity(plans.len());
    let mut index = 0;
    while index < plans.len() {
        let mode = plans[index]
            .tool
            .map_or(ToolConcurrency::Exclusive, Tool::concurrency);
        match mode {
            ToolConcurrency::Exclusive => {
                results.push(invoke(&plans[index], context).await);
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
                results.extend(run_parallel(&plans[index..end], max_parallel, context).await);
                index = end;
            }
        }
    }
    results
}
