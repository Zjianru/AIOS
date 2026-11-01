//! 大结果落盘能力（T006.1d）：挂 ToolResult，返回落盘后的 preview+locator。
//! 逻辑照 T005.2：按会话目录落盘，进日志/上下文的是 preview + locator；read 不在此列。

use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint};

pub(crate) struct SpillCapability;

impl Capability for SpillCapability {
    fn name(&self) -> &'static str {
        "spill"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::ToolResult]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::ToolResult { call, result, .. } = event else {
                return Ok(HookOutcome::Continue);
            };
            let host = event.host();
            let content = tools::bound_result(
                host.tool_session().spill(),
                &call.name,
                &result.call_id,
                &result.content,
                host.spill_max_inline_tokens(),
            )
            .unwrap_or_else(|| result.content.clone());
            Ok(HookOutcome::ToolResultContent(content))
        })
    }
}
