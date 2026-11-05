//! 接缝 2：工具级限时超时（测试清单第 13 条 stub 红测试）。

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tools::{
    Tool, ToolCall, ToolConcurrency, ToolContext, ToolFuture, ToolResult, ToolSet, execute_batch,
};

struct SlowTool {
    name: &'static str,
    wait_ms: u64,
    timeout: Option<u64>,
    mode: ToolConcurrency,
}

impl Tool for SlowTool {
    fn name(&self) -> &str {
        self.name
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
            // T009.H6：可控慢工具模拟耗时；wait_ms 由测试给定
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
        name: "slow",
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
        name: "slow",
        wait_ms: 200,
        timeout: Some(20),
        mode: ToolConcurrency::Exclusive,
    });
    let fast: Box<dyn Tool> = Box::new(SlowTool {
        name: "fast",
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
        name: "slow",
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

#[tokio::test]
async fn parallel_pool_timeout_returns_error_while_others_succeed() {
    let slow: Box<dyn Tool> = Box::new(SlowTool {
        name: "slow",
        wait_ms: 200,
        timeout: Some(20),
        mode: ToolConcurrency::Parallel,
    });
    let fast: Box<dyn Tool> = Box::new(SlowTool {
        name: "fast",
        wait_ms: 0,
        timeout: None,
        mode: ToolConcurrency::Parallel,
    });
    let set = ToolSet::new(vec![slow, fast]);
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let outcome = execute_batch(
        &set,
        &calls(&[("call_1", "slow"), ("call_2", "fast")]),
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
    assert!(!outcome.results[1].is_error);
    assert_eq!(outcome.results[1].content, "slow done");
}

struct NoDeclareTool;
impl Tool for NoDeclareTool {
    fn name(&self) -> &str {
        "nodeclare"
    }
    fn description(&self) -> &str {
        "no timeout declaration"
    }
    fn parameters(&self) -> &Value {
        static P: std::sync::OnceLock<Value> = std::sync::OnceLock::new();
        P.get_or_init(|| json!({"type": "object", "properties": {}}))
    }
    fn execute<'a>(&'a self, call: ToolCall, _c: &'a ToolContext) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move { ToolResult::success(call.id, "ok") })
    }
}

#[test]
fn canonical_arguments_ignore_object_key_order() {
    assert_eq!(
        tools::canonical_arguments("{\"b\":1,\"a\":2}"),
        tools::canonical_arguments("{\"a\":2,\"b\":1}")
    );
    assert_eq!(
        tools::canonical_arguments("{\"outer\":{\"y\":[1,2],\"x\":1}}"),
        tools::canonical_arguments("{\"outer\":{\"x\":1,\"y\":[1,2]}}")
    );
}

#[test]
fn timeout_default_is_none_and_bash_is_command_level_only() {
    let nodeclare = NoDeclareTool;
    assert_eq!(nodeclare.timeout_ms(), None, "默认 None 兼容");
    assert_eq!(
        tools::BashTool::new().timeout_ms(),
        None,
        "bash 不声明工具级限时（命令级转后台在 T005.4）"
    );
}
