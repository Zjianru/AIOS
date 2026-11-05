//! 大结果落盘能力（T006.1d）：挂 ToolResult，返回落盘后的 preview+locator。
//! 逻辑照 T005.2：按会话目录落盘，进日志/上下文的是 preview + locator；read 不在此列。

use crate::capabilities::{Capability, HookEvent, HookFuture, HookOutcome, HookPoint};
use crate::warn::warn;

pub(crate) struct SpillCapability {
    max_inline_tokens: usize,
}

impl SpillCapability {
    pub(crate) fn new(max_inline_tokens: usize) -> Self {
        Self { max_inline_tokens }
    }
}

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
            let bounded = tools::spill::bound_result_reporting(
                host.tool_session().spill(),
                &call.name,
                &result.call_id,
                &result.content,
                self.max_inline_tokens,
            );
            if let Some(warning) = bounded.warning {
                // DSH spill-policy: `ctx.logger.warn(`spill-policy: ${error}; keeping the inline content`)`；
                // AIOS 没有 logger 设施，按 skill warning 同样的约定落 stderr（T006.19）。
                warn(format!(
                    "[spill-policy] {warning}; keeping the inline content"
                ));
            }
            let content = bounded.content.unwrap_or_else(|| result.content.clone());
            Ok(HookOutcome::ToolResultContent(content))
        })
    }
}
