//! 自动压缩与上下文溢出恢复（T006.3 照 DSH compaction-basic 改回）。
//!
//! 触发：请求前用 token 估算检查是否越过预算阈值；模型报 CONTEXT_WINDOW_EXCEEDED 时强制
//! 压缩后重试；手动 /compact 在两次 turn 之间独立压缩。
//! 压缩：选一段较早历史（保留近期 tail；手动压全部可压历史），用一次额外模型请求生成
//! 摘要（复用 system+tools+前缀缓存、带输出上限），把 compaction/start + compaction/summary
//! 写进日志，再把替换写成一条带 surfaceOp replace 的 user/message（source=compact-checkpoint），
//! 最后 compaction/end；派生请求读日志消息，prompt 前缀天然命中缓存。

use std::sync::atomic::{AtomicU64, Ordering};

use model::{ContentBlock, ErrorCode, Message, MessageId, MessageSource, Role, StreamChunk, Usage};
use session::{EventKind, ShadowedRange, SurfaceOp};

use crate::capabilities::{CompactionPolicy, LoopAccess};
use crate::{AgentError, AgentEvent, CompactionOutcome, CompactionTrigger};

static COMPACTION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 压缩 checkpoint 的开头（照 DSH compaction-basic 的 checkpoint preamble）。
pub(crate) const COMPACTION_PREAMBLE: &str = "This is an automatically generated checkpoint condensing an earlier span of the conversation to free up context. Treat the captured context as established background and build on it without restating it. Continue the task directly from the messages that follow, without acknowledging this checkpoint.";
pub(crate) const SUMMARY_OPEN_TAG: &str = "<compacted-summary>";
pub(crate) const SUMMARY_CLOSE_TAG: &str = "</compacted-summary>";

/// DSH compaction-basic 的摘要指令（原样照抄；作为追加在回放历史后的最后一条 user 消息）。
const COMPACTION_INSTRUCTION: &str = "You are now acting as a compaction engine for this AI coding assistant. Condense the conversation ABOVE into a structured checkpoint that lets another model resume the work with no loss of essential context.\n\nOutput EXACTLY the Markdown structure below: keep every section, in order. Use terse bullets, not prose paragraphs. Write \"(none)\" for an empty section — never drop a section.\n\n## Primary Request and Intent\n- [the user's original and evolving goals; quote verbatim where the exact wording matters]\n\n## Key Technical Concepts\n- [technologies, frameworks, patterns, and conventions in play]\n\n## Files and Code\n- [exact path: why it matters, key changes or snippets]\n\n## Errors and Fixes\n- [error: how it was resolved, plus any related user feedback]\n\n## Pending Jobs\n- [explicitly requested work not yet completed]\n\n## Current Work\n- [precisely what was in progress at this checkpoint]\n\n## Next Step\n- [the single next action, directly in line with the most recent request, or \"(none)\"]\n\n## Critical Context\n- [decisions and their rationale, constraints, user preferences, open questions, data needed to continue]\n\nRules:\n- Write concise English engineering prose. Preserve exact file paths, commands, error strings, identifiers, numeric values, function signatures, and syntax fragments.\n- Capture user feedback and explicit instructions faithfully, especially corrections.\n- Do NOT mention this summarization request or that the context was compacted.\n- Output only the checkpoint text: do not call any tool or take any other action.\n- If the conversation already contains a <compacted-summary> block, it is a PRIOR checkpoint. Do not copy it forward verbatim: preserve still-true facts, drop stale ones, and merge newer information into a single consolidated summary under the same structure.";

/// 一次请求的压缩预算（阈值与保留比例照 DSH 公式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    pub threshold_tokens: u64,
    pub retain_tokens: u64,
}

impl Budget {
    /// 用解析后的策略、模型窗口和输出预留算阈值/保留预算。
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

fn measure(host: &dyn LoopAccess) -> u64 {
    let messages = derive_messages(host);
    estimate_messages(&messages) + estimate_schemas(host)
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

/// 摘要请求的产物：安全文本摘要、模型原始输出块、用量。
struct SummaryOutcome {
    raw_output: Vec<ContentBlock>,
    summary: Vec<ContentBlock>,
    usage: Option<Usage>,
}

/// 请求前的自动压缩：越过阈值（force=false）或溢出恢复（force=true）。
///
/// DSH 语义：压力压缩在仍超阈值时最多再压 compactionRetries 次，仍不降就记错误；
/// 溢出恢复只做一次（之后服务端还报溢出就按原错误收尾）。自动失败不打断本轮。
pub(crate) async fn compact_if_needed(
    host: &dyn LoopAccess,
    turn: u64,
    step: u64,
    trigger: CompactionTrigger,
    force: bool,
    notify_compaction: bool,
) -> Result<Option<CompactionOutcome>, AgentError> {
    let policy = host.compaction_policy();
    let budget = Budget::resolve(host);
    if !force && measure(host) < budget.threshold_tokens {
        return Ok(None);
    }
    // 确认真的要压缩后，统一派发 Compaction 挂点（自动/溢出/手动三路共用；prune 等挂这里）。
    if notify_compaction {
        host.dispatch_compaction(turn, step, trigger).await?;
    }
    let attempts = if force {
        1
    } else {
        (policy.compaction_retries as usize).saturating_add(1)
    };
    let mut outcome = None;
    for _ in 0..attempts {
        let messages = derive_messages(host);
        let Some(split) = select_split(&messages, budget.retain_tokens) else {
            if outcome.is_some() {
                return Err(AgentError::Compaction(
                    "no safe compaction range can reduce context below threshold".to_owned(),
                ));
            }
            return Ok(None);
        };
        let result = compact_once(host, Some(turn), None, trigger, split, &policy).await?;
        outcome = Some(result);
        if force || measure(host) < budget.threshold_tokens {
            return Ok(outcome);
        }
    }
    let remaining = measure(host);
    Err(AgentError::Compaction(format!(
        "compaction still above threshold after {attempts} compaction attempts ({remaining} estimated tokens >= threshold {})",
        budget.threshold_tokens
    )))
}

/// 手动 /compact（照 DSH command-compact 的 compactNow）：两次 turn 之间独立压缩，
/// 尽量压掉可压缩历史（retain=0），返回替换计数；没有可压缩历史返回 None。
pub(crate) async fn compact_now(
    host: &dyn LoopAccess,
    hook_turn: u64,
    source_command_id: Option<String>,
) -> Result<Option<CompactionOutcome>, AgentError> {
    let policy = host.compaction_policy();
    let messages = derive_messages(host);
    let Some(split) = select_split(&messages, 0) else {
        return Ok(None);
    };
    host.dispatch_compaction(hook_turn, 0, CompactionTrigger::Manual)
        .await?;
    let outcome = compact_once(
        host,
        None,
        source_command_id,
        CompactionTrigger::Manual,
        split,
        &policy,
    )
    .await?;
    Ok(Some(outcome))
}

fn derive_messages(host: &dyn LoopAccess) -> Vec<Message> {
    let store = host.session().lock().expect("agent 会话锁被毒化");
    store.derive_messages()
}

/// 执行一次完整压缩事务：start → 摘要请求 → summary → 替换消息 → end。
async fn compact_once(
    host: &dyn LoopAccess,
    turn: Option<u64>,
    source_command_id: Option<String>,
    trigger: CompactionTrigger,
    split: usize,
    policy: &CompactionPolicy,
) -> Result<CompactionOutcome, AgentError> {
    let (derived, selected) = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        let derived = store.derive_messages_with_seqs();
        let selected: Vec<Message> = derived.iter().map(|(_, message)| message.clone()).collect();
        (derived, selected)
    };
    if split < 2 || split > selected.len() {
        return Err(AgentError::Compaction(
            "invalid compaction split".to_owned(),
        ));
    }
    let shadowed_range = ShadowedRange {
        start: derived[1].0,
        end: derived[split - 1].0,
    };
    let shadowed_seqs: Vec<u64> = derived[1..split].iter().map(|(seq, _)| *seq).collect();
    let prefix: Vec<Message> = selected[..split].to_vec();
    let shadowed_token_count = estimate_messages(&prefix);

    // id 带进程/纳秒成分：续接或新进程都不会和旧 id 撞车（T005.8b）。
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.subsec_nanos());
    let compaction_id = format!(
        "compaction-{}-{nanos}-{}",
        std::process::id(),
        COMPACTION_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    let provider = host.model().provider().to_owned();
    let model = host.model().config().model.clone();

    let _ = host.events().send(AgentEvent::CompactionStarted {
        compaction_id: compaction_id.clone(),
        trigger,
    });
    // compaction/start 先落盘；拿它的 seq 拼 sourceEventSeqs。
    let start_seqs = host
        .append_with_seqs(&[EventKind::CompactionStart {
            compaction_id: compaction_id.clone(),
            source_command_id: source_command_id.clone(),
            turn,
        }])
        .await?;
    let start_seq = start_seqs[0];

    let summary = match summarize(host, &prefix, policy.summary_max_tokens).await {
        Ok(summary) => summary,
        Err(error) => {
            let message = error.to_string();
            let _ = host
                .append(&[EventKind::CompactionEnd {
                    compaction_id: compaction_id.clone(),
                    source_command_id: source_command_id.clone(),
                    turn,
                    error: Some(message.clone()),
                }])
                .await;
            let _ = host.events().send(AgentEvent::CompactionEnded {
                compaction_id,
                error: Some(message),
            });
            return Err(error);
        }
    };
    let raw_output = summary.raw_output.clone();
    let summary_blocks = summary.summary.clone();
    let usage = summary.usage.clone();

    let summary_seqs = host
        .append_with_seqs(&[EventKind::CompactionSummary {
            compaction_id: compaction_id.clone(),
            source_command_id: source_command_id.clone(),
            summary: summary_blocks.clone(),
            raw_output,
            llm_stream_call: true,
            shadowed_range,
            shadowed_seqs: shadowed_seqs.clone(),
            shadowed_token_count,
            provider,
            model,
            max_tokens: Some(policy.summary_max_tokens),
            usage,
        }])
        .await?;
    let summary_seq = summary_seqs[0];

    // 替换写成日志里的 user/message：内容 = checkpoint 框架 + 摘要块；envelope 带
    // surfaceOp replace 与 sourceEventSeqs（start、summary、被替换节点）。
    let checkpoint = checkpoint_message(
        &compaction_id,
        source_command_id.as_deref(),
        &summary_blocks,
    );
    let mut source_event_seqs = vec![start_seq, summary_seq];
    source_event_seqs.extend(shadowed_seqs.iter().copied());
    host.append_surface(
        EventKind::UserMessage {
            turn: turn.unwrap_or(0),
            step: 0,
            message: checkpoint,
        },
        SurfaceOp::Replace {
            start_seq: shadowed_range.start,
            end_seq: shadowed_range.end,
        },
        source_event_seqs,
    )
    .await?;

    host.append(&[EventKind::CompactionEnd {
        compaction_id: compaction_id.clone(),
        source_command_id,
        turn,
        error: None,
    }])
    .await?;
    let _ = host.events().send(AgentEvent::CompactionEnded {
        compaction_id,
        error: None,
    });
    Ok(CompactionOutcome {
        shadowed_items: shadowed_seqs.len(),
        shadowed_tokens: shadowed_token_count,
    })
}

/// checkpoint user 消息：DSH frameSummary 的三段（preamble+open tag / 摘要块 / close tag），
/// source 是 compact-checkpoint（与 compaction id、发起命令关联）。
fn checkpoint_message(
    compaction_id: &str,
    source_command_id: Option<&str>,
    summary: &[ContentBlock],
) -> Message {
    let mut content = vec![ContentBlock::text(format!(
        "{COMPACTION_PREAMBLE}\n\n{SUMMARY_OPEN_TAG}"
    ))];
    content.extend(summary.iter().cloned());
    content.push(ContentBlock::text(SUMMARY_CLOSE_TAG));
    Message::with_source(
        MessageId::generate(),
        Role::User,
        content,
        MessageSource::CompactCheckpoint {
            compaction_id: compaction_id.to_owned(),
            source_command_id: source_command_id.map(str::to_owned),
        },
    )
}

/// 一次摘要请求：回放被压缩的较早历史（含 system），带工具定义与输出上限，
/// 末尾追加摘要指令——system/tools/前缀与上次请求一致，命中 provider 前缀缓存。
async fn summarize(
    host: &dyn LoopAccess,
    prefix: &[Message],
    max_tokens: u32,
) -> Result<SummaryOutcome, AgentError> {
    let mut input = prefix.to_vec();
    input.push(Message::user_text(COMPACTION_INSTRUCTION));
    let tools = host.tools().definitions();
    let cancel = model::CancelToken::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    let mut stream = host
        .model()
        .stream_with_max_tokens(&input, &tools, &cancel, Some(max_tokens))
        .await?;
    let mut text = String::new();
    let mut usage: Option<Usage> = None;
    let mut finish = None;
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(StreamChunk::TextDelta { text: delta, .. }) => text.push_str(&delta),
            Ok(StreamChunk::Usage { usage: reported }) => {
                if !reported.is_empty() {
                    usage = Some(reported);
                }
            }
            Ok(StreamChunk::Finish { reason }) => finish = Some(reason),
            Ok(StreamChunk::BlockStart { .. })
            | Ok(StreamChunk::ReasoningDelta { .. })
            | Ok(StreamChunk::BlockEnd { .. }) => {}
            Err(error) => return Err(AgentError::Model(error)),
        }
    }
    if let Some(reason) = finish
        && reason == model::FinishReason::MaxTokens
    {
        return Err(AgentError::Model(model::ModelError::new(
            ErrorCode::InvalidRequest,
            "summarization truncated at the token cap (incomplete checkpoint)".to_owned(),
        )));
    }
    let text = text.trim().to_owned();
    if text.is_empty() {
        return Err(AgentError::Model(model::ModelError::new(
            ErrorCode::EmptyResponse,
            "压缩请求没有返回摘要文本".to_owned(),
        )));
    }
    let block = ContentBlock::text(text);
    Ok(SummaryOutcome {
        raw_output: vec![block.clone()],
        summary: vec![block],
        usage,
    })
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
