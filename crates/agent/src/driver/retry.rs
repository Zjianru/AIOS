#![allow(unused_imports)]
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use model::{
    AssistantStreamAccumulator, BlockType, CancelToken, ContentBlock, CredentialContext, DeepSeek,
    ErrorCode, FinishReason, Message, ModelError, StreamChunk, TimedStreamChunk, Usage,
    UserMessage,
};
use session::{
    AbortCause, EventKind, InboxTarget, LlmFailure, RetryId, RetryMode, SessionId, SessionStore,
    ToolError, TurnEndReason,
};
use tokio::sync::Notify;
use tools::{ToolCall, ToolContext, ToolSessionState, ToolSet, execute_batch};

use crate::capabilities::{
    BoxedFuture, CapabilityRegistry, CompactionPolicy, HookEvent, LoopAccess,
};
use crate::error::AgentError;
use crate::inbox::{Delivery, Inbox};
use crate::{AgentConfig, AgentEvent, CompactionTrigger, Deps, TurnOutcome};

use super::*;

/// 请求层重试：最多 5 次、500ms 起指数退避、上限 10s、±10% 抖动（std 派生，不加依赖）。
pub(super) const MAX_RETRIES: u32 = 5;
pub(super) const INITIAL_RETRY_DELAY_MS: u64 = 500;
pub(super) const MAX_RETRY_DELAY_MS: u64 = 10_000;
pub(super) const RETRY_JITTER_RATIO: f64 = 0.1;

/// spec 定的 5 个可重试码（排序后写进 policyKey，照 DSH retryPolicyKey）。
pub(super) const RETRYABLE_CODES: [ErrorCode; 5] = [
    ErrorCode::EmptyResponse,
    ErrorCode::RateLimit,
    ErrorCode::Server,
    ErrorCode::Timeout,
    ErrorCode::Transport,
];

/// 固定重试策略的稳定标识（DSH llm-retry/src/index.ts:70-80 的 retryPolicyKey）。
pub(super) fn retry_policy_key() -> String {
    let mut codes: Vec<String> = RETRYABLE_CODES.iter().map(|code| code.as_str()).collect();
    codes.sort();
    serde_json::json!([
        "normal",
        MAX_RETRIES,
        codes,
        INITIAL_RETRY_DELAY_MS,
        MAX_RETRY_DELAY_MS,
        RETRY_JITTER_RATIO,
    ])
    .to_string()
}

pub(super) enum RetryDecision {
    Retry,
    Cancel,
    Fatal,
}

/// 只认 spec 定的 5 个可重试码。
pub(super) fn is_retryable(code: ErrorCode) -> bool {
    RETRYABLE_CODES.contains(&code)
}

/// 这次失败要不要重试；要的话等完（可取消）再回 Retry。
///
/// T006.2b 照 DSH：已流出可见内容也重试（半截内容只留在 assistant/attempt 的 stream 里）；
/// 排定的重试先持久化 `llm/retry` 再做可取消等待，等待结束后、下一次尝试前落
/// `llm/retry-started`（DSH llm-retry/src/index.ts:166-186）。
pub(super) async fn decide_retry(
    inner: &Arc<Inner>,
    cancel: &CancelToken,
    error: &ModelError,
    turn: u64,
    step: u64,
    attempt: &mut u32,
    retry_id: &mut Option<RetryId>,
) -> Result<RetryDecision, AgentError> {
    if !is_retryable(error.code) || *attempt >= MAX_RETRIES {
        return Ok(RetryDecision::Fatal);
    }
    let Some(delay_ms) = retry_delay_ms(error, *attempt + 1) else {
        return Ok(RetryDecision::Fatal);
    };
    *attempt += 1;
    // 同一条重试链共用一个 retryId（DSH 投影在 step/start、turn/end 后重置）。
    let retry_id = retry_id.get_or_insert_with(RetryId::generate).clone();
    // durable before the cancellable wait。
    inner
        .append(&[EventKind::LlmRetry {
            retry_id: retry_id.clone(),
            turn,
            step,
            provider: inner.model.provider().to_owned(),
            mode: RetryMode::Normal,
            policy_key: retry_policy_key(),
            retry: *attempt,
            max_retries: Some(MAX_RETRIES),
            delay_ms,
            failure: LlmFailure {
                message: error.message.clone(),
                code: error.code.as_str(),
            },
        }])
        .await?;
    let _ = inner.events.send(AgentEvent::Retrying {
        attempt: *attempt,
        max_attempts: MAX_RETRIES,
        delay_ms,
        code: error.code.as_str(),
    });
    let cancelled = tokio::select! {
        biased;
        () = cancel.cancelled() => true,
        () = tokio::time::sleep(Duration::from_millis(delay_ms)) => false,
    };
    if cancelled {
        return Ok(RetryDecision::Cancel);
    }
    // 等待结束、下一次尝试开始前：llm/retry-started（DSH index.ts:184-186）。
    inner
        .append(&[EventKind::LlmRetryStarted {
            retry_id,
            turn,
            step,
            retry: *attempt,
        }])
        .await?;
    Ok(RetryDecision::Retry)
}

/// 服务端 retry-after 优先；超过上限就不重试（照 DSH normal 模式）；否则本地退避。
pub(super) fn retry_delay_ms(error: &ModelError, retry: u32) -> Option<u64> {
    if let Some(after) = error.retry_after_ms {
        if after > MAX_RETRY_DELAY_MS {
            return None;
        }
        return Some(after.max(1));
    }
    let exponent = retry.saturating_sub(1).min(10);
    let exponential = INITIAL_RETRY_DELAY_MS
        .saturating_mul(1u64 << exponent)
        .min(MAX_RETRY_DELAY_MS);
    let factor = jitter_factor(retry);
    Some(((exponential as f64 * factor).round() as u64).clamp(1, MAX_RETRY_DELAY_MS))
}

/// std 派生伪随机抖动（SystemTime 纳秒 + 重试序号混合），不引 rand。
pub(super) fn jitter_factor(retry: u32) -> f64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.subsec_nanos() as u64);
    let mixed = nanos ^ u64::from(retry).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let unit = (mixed >> 11) as f64 / (1u64 << 53) as f64;
    1.0 + 0.1 * (2.0 * unit - 1.0)
}

pub(super) fn reset_attempt(
    blocks: &mut BTreeMap<usize, ContentBlock>,
    open: &mut BTreeMap<usize, (BlockType, String)>,
    usage: &mut Option<Usage>,
    finish: &mut Option<FinishReason>,
    stream: &mut AssistantStreamAccumulator,
) {
    blocks.clear();
    open.clear();
    *usage = None;
    *finish = None;
    *stream = AssistantStreamAccumulator::new();
}
