//! 接缝 2：工具级限时超时（测试清单第 13 条 stub 红测试）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tools::{
    Tool, ToolCall, ToolConcurrency, ToolContext, ToolFuture, ToolResult, ToolSet, execute_batch,
};

struct SlowTool {
    wait_ms: u64,
    timeout: Option<u64>,
    mode: ToolConcurrency,
}

impl Tool for SlowTool {
    fn name(&self) -> &str {
        "slow"
    }
    fn description(&self) -> &str {
        "test slow tool"
    }
    fn parameters(&self) -> &Value {
        static PARAMS: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        PARAMS.get_or_init(|| json!({"type": "object", "properties": {}}))
    }
    fn timeout_ms(&self) -> Option<u64> {
        self.timeout
    }
    fn concurrency(&self) -> ToolConcurrency {
        self.mode
    }
    fn execute<'a>(
        &'a self,
        call: ToolCall,
        _context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(self.wait_ms)).await;
            ToolResult::success(call.id, "slow done")
        })
    }
}

fn context() -> ToolContext {
    let dir = std::env::temp_dir();
    ToolContext::with_session(dir, Arc::new(tools::ToolSessionState::detached()))
}

fn calls(names: &[(&str, &str)]) -> Vec<ToolCall> {
    names
        .iter()
        .map(|(id, name)| ToolCall {
            id: (*id).to_owned(),
            name: (*name).to_owned(),
            arguments: "{}".to_owned(),
        })
        .collect()
}

#[tokio::test]
async fn timed_out_tool_returns_timeout_error() {
    let tool: Box<dyn Tool> = Box::new(SlowTool {
        wait_ms: 200,
        timeout: Some(20),
        mode: ToolConcurrency::Exclusive,
    });
    let set = ToolSet::new(vec![tool]);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let started = Instant::now();
    let outcome = execute_batch(&set, &calls(&[("call_1", "slow")]), 4, &context(), &cancel).await;
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "超时不应等工具跑完"
    );
    let result = &outcome.results[0];
    assert!(result.is_error);
    assert_eq!(result.error_code.as_deref(), Some("TOOL_TIMEOUT"));
    assert_eq!(result.content, "Error: tool call timed out after 20ms");
}

#[tokio::test]
async fn exclusive_timed_out_tool_does_not_block_batch() {
    let slow: Box<dyn Tool> = Box::new(SlowTool {
        wait_ms: 200,
        timeout: Some(20),
        mode: ToolConcurrency::Exclusive,
    });
    let fast: Box<dyn Tool> = Box::new(SlowTool {
        wait_ms: 0,
        timeout: None,
        mode: ToolConcurrency::Parallel,
    });
    // 两个同名工具：用 ToolSet 里第二个不同名字不方便，直接验证独占超时后批继续。
    let set = ToolSet::new(vec![slow]);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let outcome = execute_batch(
        &set,
        &calls(&[("call_1", "slow"), ("call_2", "unknown")]),
        4,
        &context(),
        &cancel,
    )
    .await;
    assert_eq!(outcome.results.len(), 2);
    assert!(outcome.results[0].is_error);
    assert_eq!(
        outcome.results[0].error_code.as_deref(),
        Some("TOOL_TIMEOUT")
    );
    assert_eq!(
        outcome.results[1].error_code.as_deref(),
        Some("TOOL_NOT_FOUND")
    );
    let _ = fast;
}

#[tokio::test]
async fn tool_without_timeout_is_unchanged() {
    let tool: Box<dyn Tool> = Box::new(SlowTool {
        wait_ms: 30,
        timeout: None,
        mode: ToolConcurrency::Exclusive,
    });
    let set = ToolSet::new(vec![tool]);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let outcome = execute_batch(&set, &calls(&[("call_1", "slow")]), 4, &context(), &cancel).await;
    assert!(!outcome.results[0].is_error);
    assert_eq!(outcome.results[0].content, "slow done");
}
