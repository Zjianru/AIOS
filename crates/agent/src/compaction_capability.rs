//! 压缩能力：把 compaction.rs 的实现挂到 loop 的固定挂点上（T006.1b）。
//!
//! 挂点语义：PreStep = 每步自动检查（trigger=None）或手动 /compact（trigger=Some(Manual)）；
//! RequestError = 溢出后强制压缩重试一次；Compaction 由 compaction::compact_if_needed
//! 在确认后统一派发（供 prune 等，本能力不消费，避免自递归）。
//! 日志与事件全部走 LoopAccess::append / events（SessionEvent 由 append 派发）。

use model::ErrorCode;

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};
use crate::compaction;
use crate::{AgentError, CompactionTrigger};

pub(crate) struct CompactionCapability;

impl Capability for CompactionCapability {
    fn name(&self) -> &'static str {
        "compaction"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[
            HookPoint::PreStep,
            HookPoint::Request,
            HookPoint::RequestError,
        ]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            match event {
                // 每步之前：trigger=None 是自动检查（越过阈值才压）；
                // Some(Manual) 是手动 /compact 入口（强制压）。两路失败都不打断本轮（同旧路径）。
                HookEvent::PreStep { trigger, phase, .. } if *phase == PreStepPhase::Compact => {
                    let (force, trigger) = match trigger {
                        Some(trigger) => (true, *trigger),
                        None => (false, CompactionTrigger::Auto),
                    };
                    let _ = compaction::compact_if_needed(
                        event.host(),
                        event.turn(),
                        event.step(),
                        trigger,
                        force,
                        true,
                    )
                    .await;
                }
                // 发请求时：token 记账留给后续票（现在只占位，不改变行为）。
                HookEvent::Request { .. } => {}
                // 请求报错：溢出时强制压缩。
                // 语义：compact_if_needed 返回 Err（如摘要失败）时原样向上返回压缩错误；
                // 只有 Ok(false)（无前缀可压）才返回原始 model 错误（行为同旧路径）。
                HookEvent::RequestError { error, .. }
                    if error.code == ErrorCode::ContextWindowExceeded =>
                {
                    let compacted = compaction::compact_if_needed(
                        event.host(),
                        event.turn(),
                        event.step(),
                        CompactionTrigger::Auto,
                        true,
                        true,
                    )
                    .await?;
                    if !compacted {
                        return Err(AgentError::Model(model::ModelError {
                            code: error.code,
                            message: error.message.clone(),
                            status: error.status,
                            retry_after_ms: error.retry_after_ms,
                        }));
                    }
                }
                _ => {}
            }
            Ok(HookOutcome::Continue)
        })
    }
}
