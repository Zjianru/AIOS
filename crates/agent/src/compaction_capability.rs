//! 压缩能力：把 compaction.rs 的实现挂到 loop 的固定挂点上（T006.1b）。
//!
//! 挂点语义：PreStep = 每步请求前的自动检查（should_trigger/select_split 确认后才真正压缩）；
//! RequestError = 溢出后强制压缩重试一次；Compaction = 已确认的压缩执行入口（手动 /compact）。
//! 日志与事件全部走 LoopAccess::append / events（SessionEvent 由 append 派发）。

use model::ErrorCode;

use crate::capabilities::{Capability, HookEvent, HookFuture, HookPoint};
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
            HookPoint::Compaction,
        ]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            match event {
                // 每步之前：越过阈值就压缩较早历史；不越过则 no-op。
                HookEvent::PreStep { .. } => {
                    // 行为同旧路径：自动压缩失败不打断本轮（事件已记录），调用方只推进。
                    let _ = compaction::compact_if_needed(
                        event.host(),
                        event.turn(),
                        event.step(),
                        CompactionTrigger::Auto,
                        false,
                    )
                    .await;
                }
                // 发请求时：token 记账留给后续票（现在只占位，不改变行为）。
                HookEvent::Request { .. } => {}
                // 请求报错：溢出时强制压缩；压缩不了把原始错误还回去（行为同旧路径）。
                HookEvent::RequestError { error, .. } => {
                    if error.code == ErrorCode::ContextWindowExceeded {
                        let compacted = compaction::compact_if_needed(
                            event.host(),
                            event.turn(),
                            event.step(),
                            CompactionTrigger::Auto,
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
                }
                // 已确认的压缩执行入口（手动 /compact）：强制压缩一次。
                HookEvent::Compaction { trigger, .. } => {
                    compaction::compact_if_needed(
                        event.host(),
                        event.turn(),
                        event.step(),
                        *trigger,
                        true,
                    )
                    .await?;
                }
                _ => {}
            }
            Ok(())
        })
    }
}
