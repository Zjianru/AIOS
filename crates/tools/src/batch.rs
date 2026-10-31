//! 一次 step 的工具批执行（骨架）。
//!
//! 实现提交里补：独占调用是屏障、并行调用进有界滚动池、结果按模型顺序提交。
//! 骨架阶段先把所有调用同时放出去，不设上限、不分屏障（测试会红）。

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

use crate::{Tool, ToolCall, ToolContext, ToolResult, ToolSet};

struct Planned<'a> {
    call: &'a ToolCall,
    tool: Option<&'a dyn Tool>,
}

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

/// 执行一次 step 的全部工具调用；骨架实现把全部调用同时放出去。
pub async fn execute_batch(
    tools: &ToolSet,
    calls: &[ToolCall],
    _max_parallel: usize,
    context: &ToolContext,
) -> Vec<ToolResult> {
    let plans: Vec<Planned<'_>> = calls
        .iter()
        .map(|call| Planned {
            call,
            tool: tools.get(&call.name),
        })
        .collect();
    let mut results: Vec<Option<ToolResult>> = plans.iter().map(|_| None).collect();
    let mut futures: Vec<PendingFuture<'_, ToolResult>> = Vec::new();
    for (index, plan) in plans.iter().enumerate() {
        futures.push((index, Box::pin(invoke(plan, context))));
    }
    while !futures.is_empty() {
        let (position, result) = next_finished(&mut futures).await;
        let (index, _) = futures.remove(position);
        results[index] = Some(result);
    }
    results
        .into_iter()
        .map(|result| result.expect("并行池没有漏收结果"))
        .collect()
}
