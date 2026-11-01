//! 自动压缩与上下文溢出恢复（照 DSH compaction-basic 的可观察语义做最小实现）。
//!
//! 触发：请求前用 token 估算检查是否越过预算阈值；模型报 CONTEXT_WINDOW_EXCEEDED 时强制压缩后重试一次。
//! 压缩：选一段较早历史（保留近期 tail），用一次额外模型请求生成摘要，把
//! compaction/start + compaction/summary + compaction/end 写进日志（只追加）；派生请求由
//! derive 用一条 checkpoint 消息替换被压缩的 seq 区间。原始日志保留、可回放。

use std::sync::atomic::{AtomicU64, Ordering};

use model::{ContentBlock, ErrorCode, Message, Role, StreamChunk};
use session::EventKind;

use crate::capabilities::LoopAccess;
use crate::{AgentError, AgentEvent, CompactionTrigger};

static COMPACTION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// DSH compaction-basic 的摘要指令（原样照抄；作为追加在回放历史后的最后一条 user 消息）。
const COMPACTION_INSTRUCTION: &str = "You are now acting as a compaction engine for this AI coding assistant. Condense the conversation ABOVE into a structured checkpoint that lets another model resume the work with no loss of essential context.\n\nOutput EXACTLY the Markdown structure below: keep every section, in order. Use terse bullets, not prose paragraphs. Write \"(none)\" for an empty section — never drop a section.\n\n## Primary Request and Intent\n- [the user's original and evolving goals; quote verbatim where the exact wording matters]\n\n## Key Technical Concepts\n- [technologies, frameworks, patterns, and conventions in play]\n\n## Files and Code\n- [exact path: why it matters, key changes or snippets]\n\n## Errors and Fixes\n- [error: how it was resolved, plus any related user feedback]\n\n## Pending Jobs\n- [explicitly requested work not yet completed]\n\n## Current Work\n- [precisely what was in progress at this checkpoint]\n\n## Next Step\n- [the single next action, directly in line with the most recent request, or \"(none)\"]\n\n## Critical Context\n- [decisions and their rationale, constraints, user preferences, open questions, data needed to continue]\n\nRules:\n- Write concise English engineering prose. Preserve exact file paths, commands, error strings, identifiers, numeric values, function signatures, and syntax fragments.\n- Capture user feedback and explicit instructions faithfully, especially corrections.\n- Do NOT mention this summarization request or that the context was compacted.\n- Output only the checkpoint text: do not call any tool or take any other action.\n- If the conversation already contains a <compacted-summary> block, it is a PRIOR checkpoint. Do not copy it forward verbatim: preserve still-true facts, drop stale ones, and merge newer information into a single consolidated summary under the same structure.";

/// 一次请求的压缩预算（阈值与保留比例照 DSH 公式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    pub threshold_tokens: u64,
    pub retain_tokens: u64,
}

impl Budget {
    /// 用模型表窗口、输出预留、headroom 和 AgentConfig 比例算阈值/保留预算。
    /// AIOS 偏离：窗口/输出/headroom 放不下时（W−O−B≤0）DSH 会在加载时报错，
    /// AIOS 退回 floor(W×thresholdRatio) 保证默认配置可用（handback 记偏离）。
    pub(crate) fn resolve(host: &dyn LoopAccess) -> Self {
        let config = host.model().config();
        let window = config.context_window;
        let output = u64::from(config.max_tokens);
        let policy = host.compaction_policy();
        let headroom = policy.headroom_tokens;
        let message_budget = window.saturating_sub(output);
        let ratio_threshold = ((window as f64) * policy.threshold_ratio) as u64;
        let threshold_tokens = if message_budget > headroom {
            ratio_threshold.min(message_budget - headroom)
        } else {
            ratio_threshold
        };
        let threshold_tokens = threshold_tokens.max(1);
        let retain_base = if message_budget > 0 {
            message_budget
        } else {
            window / 2
        };
        let retain_tokens = (((retain_base as f64) * policy.retain_ratio) as u64)
            .max(1)
            .min(threshold_tokens.saturating_sub(1).max(1));
        Self {
            threshold_tokens,
            retain_tokens,
        }
    }
}

/// 估算一条消息的 token（照 T005.2/DSH token-meter 的 chars/4+4 近似）。
fn estimate_message(message: &Message) -> u64 {
    let mut total = 4_u64;
    for block in &message.content {
        match block {
            ContentBlock::Text { text } | ContentBlock::Reasoning { text, .. } => {
                total += tools::estimate_text_tokens(text) as u64;
            }
            ContentBlock::ToolCall {
                name, arguments, ..
            } => {
                total += tools::estimate_text_tokens(name) as u64;
                total += tools::estimate_text_tokens(arguments) as u64;
            }
        }
    }
    total
}

/// 估算派生消息的 token 总数（压缩阈值用）。
fn estimate_messages(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message).sum()
}

fn estimate_schemas(host: &dyn LoopAccess) -> u64 {
    let schemas = host.tools().definitions();
    let mut total = 0_usize;
    for schema in &schemas {
        total += schema.name.chars().count();
        total += schema.description.chars().count();
        total += schema.parameters.to_string().chars().count();
    }
    (total.div_ceil(4) + 4) as u64
}

/// 选保留 tail 的起点（消息下标，>=1）：从末尾按 retain 预算回退，且不把 tool/result
/// 与其前面的 assistant tool-call 拆开。返回 None 表示没有可压缩的较早历史。
pub(crate) fn select_split(messages: &[Message], retain_tokens: u64) -> Option<usize> {
    if messages.len() < 3 {
        return None;
    }
    let mut index = messages.len();
    let mut used = 0_u64;
    while index > 1 {
        let cost = estimate_message(&messages[index - 1]);
        if used + cost > retain_tokens {
            break;
        }
        used += cost;
        index -= 1;
    }
    // 至少保留最后一条消息。
    let mut split = index.min(messages.len() - 1);
    // 保留 tail 不能以 tool/result 开头：把结果并进摘要侧。
    while split > 1 && messages[split].role == Role::Tool {
        split -= 1;
    }
    if split > 1 { Some(split) } else { None }
}

fn should_trigger(host: &dyn LoopAccess, messages: &[Message], budget: Budget) -> bool {
    let total = estimate_messages(messages) + estimate_schemas(host);
    total >= budget.threshold_tokens
}

/// 请求前的自动压缩：越过阈值（force=false）或溢出恢复（force=true）时压缩一次。
/// 返回是否真的压缩（None 变化 / 失败时调用方决定是否继续）。
pub(crate) async fn compact_if_needed(
    host: &dyn LoopAccess,
    turn: u64,
    step: u64,
    trigger: CompactionTrigger,
    force: bool,
    notify_compaction: bool,
) -> Result<bool, AgentError> {
    let budget = Budget::resolve(host);
    let (derived, selected) = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        let derived = store.derive_messages_with_seqs();
        let messages: Vec<Message> = derived.iter().map(|(_, message)| message.clone()).collect();
        (derived, messages)
    };
    if !force && !should_trigger(host, &selected, budget) {
        return Ok(false);
    }
    let Some(split) = select_split(&selected, budget.retain_tokens) else {
        return Ok(false);
    };
    // T006.1b：确认真的要压缩后，统一派发 Compaction 挂点（自动/溢出/手动三路共用）；
    // prune 等后续功能挂在这里，T006.4 搬入；CompactionCapability 自己不消费这个挂点，避免递归。
    if notify_compaction {
        host.dispatch_compaction(turn, step, trigger).await?;
    }
    // 系统消息永远保留；被摘要的是 prefix 去掉系统后的部分。
    let shadowed_range = session::ShadowedRange {
        start: derived[1].0,
        end: derived[split - 1].0,
    };
    let shadowed_seqs: Vec<u64> = derived[1..split].iter().map(|(seq, _)| *seq).collect();
    let prefix: Vec<Message> = selected[..split].to_vec();
    let shadowed_token_count = estimate_messages(&prefix);
    let provider = host.model().provider().to_owned();
    let model = host.model().config().model.clone();
    // T005.8b：id 带进程/纳秒成分，续接或新进程都不会和旧 compaction-1 撞车。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    let compaction_id = format!(
        "compaction-{}-{nanos}-{}",
        std::process::id(),
        COMPACTION_COUNTER.fetch_add(1, Ordering::Relaxed)
    );

    let _ = host.events().send(AgentEvent::CompactionStarted {
        compaction_id: compaction_id.clone(),
        trigger,
    });
    host.append(&[EventKind::CompactionStart {
        compaction_id: compaction_id.clone(),
        source_command_id: None,
        turn: Some(turn),
    }])
    .await?;

    let summary = match summarize(host, &prefix).await {
        Ok(summary) => summary,
        Err(error) => {
            let message = error.to_string();
            host.append(&[EventKind::CompactionEnd {
                compaction_id: compaction_id.clone(),
                source_command_id: None,
                turn: Some(turn),
                error: Some(message.clone()),
            }])
            .await?;
            let _ = host.events().send(AgentEvent::CompactionEnded {
                compaction_id,
                error: Some(message),
            });
            return Err(error);
        }
    };

    let summary_blocks = vec![ContentBlock::text(summary)];
    host.append(&[
        EventKind::CompactionSummary {
            compaction_id: compaction_id.clone(),
            source_command_id: None,
            summary: summary_blocks.clone(),
            raw_output: summary_blocks.clone(),
            llm_stream_call: true,
            shadowed_range,
            shadowed_seqs,
            shadowed_token_count,
            provider,
            model,
            max_tokens: None,
            usage: None,
        },
        EventKind::CompactionEnd {
            compaction_id: compaction_id.clone(),
            source_command_id: None,
            turn: Some(turn),
            error: None,
        },
    ])
    .await?;
    let _ = host.events().send(AgentEvent::CompactionEnded {
        compaction_id,
        error: None,
    });
    Ok(true)
}

/// 一次摘要请求：回放被压缩的较早历史 + 摘要指令，收集文本输出。
async fn summarize(host: &dyn LoopAccess, prefix: &[Message]) -> Result<String, AgentError> {
    let mut input = prefix.to_vec();
    input.push(Message::user_text(COMPACTION_INSTRUCTION));
    let cancel = model::CancelToken::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    let mut stream = host.model().stream(&input, &[], &cancel).await?;
    let mut summary = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(StreamChunk::TextDelta { text, .. }) => summary.push_str(&text),
            Ok(_) => {}
            Err(error) => return Err(AgentError::Model(error)),
        }
    }
    let summary = summary.trim().to_owned();
    if summary.is_empty() {
        return Err(AgentError::Model(model::ModelError::new(
            ErrorCode::EmptyResponse,
            "压缩请求没有返回摘要文本".to_owned(),
        )));
    }
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn message(text: &str) -> Message {
        Message::user_text(text)
    }

    #[test]
    fn estimate_is_chars_over_four_plus_overhead() {
        let one = message("abcd");
        assert_eq!(estimate_message(&one), 4 + 5, "chars/4+4 文本 + 4 消息开销");
    }

    #[test]
    fn split_keeps_recent_tail_and_requires_prefix() {
        let messages = vec![
            Message::system_prompt("system"),
            message(&"a".repeat(400)),
            message(&"b".repeat(400)),
            message(&"c".repeat(400)),
        ];
        let split = select_split(&messages, 150).expect("应能压缩较早历史");
        assert!(split >= 2, "至少保留最后一条消息：{split}");
        assert_eq!(
            select_split(&messages[..2], 150),
            None,
            "只有系统+一条时不压"
        );
    }
}
