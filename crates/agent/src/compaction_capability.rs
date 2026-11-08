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

pub(crate) struct CompactionCapability {
    /// T006.F1：手动压缩互斥（照 command-compact busy；不排队）。
    manual_compaction: std::sync::atomic::AtomicBool,
}

impl CompactionCapability {
    pub(crate) fn new() -> Self {
        Self {
            manual_compaction: std::sync::atomic::AtomicBool::new(false),
        }
    }
}

fn manual_busy_error() -> AgentError {
    AgentError::ManualCompaction {
        code: crate::ManualCompactionErrorCode::Busy,
        message: crate::ManualCompactionErrorCode::Busy.text().to_owned(),
    }
}

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

    fn compact_manual_with<'a>(
        &'a self,
        host: &'a dyn crate::capabilities::LoopAccess,
        source_command_id: Option<String>,
    ) -> crate::capabilities::BoxedFuture<
        'a,
        Option<Result<Option<crate::CompactionOutcome>, AgentError>>,
    > {
        Box::pin(async move {
            if host.is_running() {
                return Ok(Some(Err(manual_busy_error())));
            }
            if self
                .manual_compaction
                .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                return Ok(Some(Err(manual_busy_error())));
            }
            let result = compaction::compact_now(host, host.last_turn(), source_command_id)
                .await
                .map_err(|error| {
                    let failure = compaction::manual_failure(error);
                    AgentError::ManualCompaction {
                        code: failure.code,
                        message: failure.message,
                    }
                });
            self.manual_compaction
                .store(false, std::sync::atomic::Ordering::SeqCst);
            Ok(Some(result))
        })
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            match event {
                // 每步之前：自动检查（越过阈值才压）；手动 /compact 由 driver 直接走 compact_now，
                // 不经过 PreStep（避免顺带注入指令/skill）。自动失败不打断本轮（事件已记录）。
                HookEvent::PreStep {
                    trigger: None,
                    phase,
                    ..
                } if *phase == PreStepPhase::Compact => {
                    #[expect(
                        clippy::let_underscore_must_use,
                        reason = "自动压缩失败不打断本轮（事件已记录），这里只触发"
                    )]
                    let _ = compaction::compact_if_needed(
                        event.host(),
                        event.turn(),
                        event.step(),
                        CompactionTrigger::Auto,
                        false,
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
                    // DSH：溢出恢复失败/没有可压缩历史时保留原始请求错误（不把压缩错误
                    // 当成请求错误），压缩尝试自己的失败已经写进 compaction/end。
                    let original_error = || {
                        AgentError::Model(model::ModelError {
                            code: error.code,
                            message: error.message.clone(),
                            status: error.status,
                            retry_after_ms: error.retry_after_ms,
                        })
                    };
                    match compaction::compact_if_needed(
                        event.host(),
                        event.turn(),
                        event.step(),
                        CompactionTrigger::Auto,
                        true,
                        true,
                    )
                    .await
                    {
                        Ok(Some(_)) => {}
                        Ok(None) => return Err(original_error()),
                        Err(_) => return Err(original_error()),
                    }
                }
                _ => {}
            }
            Ok(HookOutcome::Continue)
        })
    }
}
