//! 自动压缩与上下文溢出恢复（照 DSH compaction-basic 改回）。
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
use crate::error::ManualCompactionErrorCode;
use crate::{AgentError, AgentEvent, CompactionOutcome, CompactionTrigger};

/// 手动压缩失败分类（不排队；Agent::compact 转为 AgentError::ManualCompaction）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManualCompactionFailure {
    pub(crate) code: ManualCompactionErrorCode,
    pub(crate) message: String,
}

/// 把一次手动压缩错误归类成 DSH 的 ManualCompactionErrorCode
/// （compaction/src/index.ts:35-46；文案映射在 command-compact expectedFailure）。
pub(crate) fn manual_failure(error: AgentError) -> ManualCompactionFailure {
    match error {
        AgentError::ManualCompaction { code, message } => ManualCompactionFailure { code, message },
        AgentError::Model(error) => ManualCompactionFailure {
            code: ManualCompactionErrorCode::Summary,
            message: error.to_string(),
        },
        AgentError::Store(error) => ManualCompactionFailure {
            code: ManualCompactionErrorCode::Persistence,
            message: error.to_string(),
        },
        other => ManualCompactionFailure {
            code: ManualCompactionErrorCode::Commit,
            message: other.to_string(),
        },
    }
}

/// 手动路径的错误包装：manual 时带 DSH 分类，自动路径保留原错误形状（不改变自动压缩行为）。
fn manual_error(
    trigger: CompactionTrigger,
    code: ManualCompactionErrorCode,
    error: AgentError,
) -> AgentError {
    if trigger == CompactionTrigger::Manual {
        AgentError::ManualCompaction {
            code,
            message: error.to_string(),
        }
    } else {
        error
    }
}

static COMPACTION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// 压缩 checkpoint 的开头（照 DSH compaction-basic 的 checkpoint preamble）。
pub(crate) const COMPACTION_PREAMBLE: &str = "This is an automatically generated checkpoint condensing an earlier span of the conversation to free up context. Treat the captured context as established background and build on it without restating it. Continue the task directly from the messages that follow, without acknowledging this checkpoint.";
pub(crate) const SUMMARY_OPEN_TAG: &str = "<compacted-summary>";
pub(crate) const SUMMARY_CLOSE_TAG: &str = "</compacted-summary>";

/// DSH compaction-basic 的摘要指令（原样照抄；作为追加在回放历史后的最后一条 user 消息）。
const KIND_COMPACTION_INSTRUCTION: &str = "You are now acting as a compaction engine for AIOS Kind, the coordinator agent. Condense the conversation ABOVE into a structured checkpoint that lets another model resume the work with no loss of essential context.\n\nKeep exactly these seven sections, in order, terse bullets, write \"(none)\" when empty:\n\n## 在办的事和对谁的承诺\n- [what is in progress and who was promised what]\n\n## 在等什么、等谁\n- [waiting on what/who]\n\n## 任务 id 和状态\n- [task id: state]\n\n## 最近用户原话\n- [messageId: verbatim user words that matter]\n\n## 已定下的约定\n- [settled conventions/decisions]\n\n## 产出文件\n- [id/path only; do not paste file bodies]\n\n## 下一步\n- [single next action]\n\nRules: drop finished process and raw tool output; when something can be read again by id or path, keep only its id/path; never invent facts or ids; do not mention that this is a summarization request; output only the checkpoint. 每条要么给出可见 id（messageId、任务 id、文件路径），要么明确写「转述」；不要编 id，不要用序号。";

const COMPACTION_INSTRUCTION: &str = "You are now acting as a compaction engine for this AI coding assistant. Condense the conversation ABOVE into a structured checkpoint that lets another model resume the work with no loss of essential context.\n\nOutput EXACTLY the Markdown structure below: keep every section, in order. Use terse bullets, not prose paragraphs. Write \"(none)\" for an empty section — never drop a section.\n\n## Primary Request and Intent\n- [the user's original and evolving goals; quote verbatim where the exact wording matters]\n\n## Key Technical Concepts\n- [technologies, frameworks, patterns, and conventions in play]\n\n## Files and Code\n- [exact path: why it matters, key changes or snippets]\n\n## Errors and Fixes\n- [error: how it was resolved, plus any related user feedback]\n\n## Pending Jobs\n- [explicitly requested work not yet completed]\n\n## Current Work\n- [precisely what was in progress at this checkpoint]\n\n## Next Step\n- [the single next action, directly in line with the most recent request, or \"(none)\"]\n\n## Critical Context\n- [decisions and their rationale, constraints, user preferences, open questions, data needed to continue]\n\nRules:\n- Write concise English engineering prose. Preserve exact file paths, commands, error strings, identifiers, numeric values, function signatures, and syntax fragments.\n- Capture user feedback and explicit instructions faithfully, especially corrections.\n- Do NOT mention this summarization request or that the context was compacted.\n- Output only the checkpoint text: do not call any tool or take any other action.\n- If the conversation already contains a <compacted-summary> block, it is a PRIOR checkpoint. Do not copy it forward verbatim: preserve still-true facts, drop stale ones, and merge newer information into a single consolidated summary under the same structure.\n- 每条要么给出可见 id（messageId、任务 id、文件路径），要么明确写「转述」；不要编 id，不要用序号。";

/// 一次请求的压缩预算（阈值与保留比例照 DSH 公式）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Budget {
    pub threshold_tokens: u64,
    pub retain_tokens: u64,
}

impl Budget {
    /// 用解析后的策略、模型窗口和输出预留算阈值/保留预算。
    pub(crate) fn resolve(host: &dyn LoopAccess) -> Self {
        let model = host.model();
        let window = model.context_window();
        let output = u64::from(model.config().max_tokens);
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

/// 估算一条消息的 token（照 DSH token-meter 的 chars/4+4 近似）。
pub(crate) fn estimate_message(message: &Message) -> u64 {
    let mut total = 4_u64;
    for block in &message.content {
        match block {
            ContentBlock::Text { text, .. } | ContentBlock::Reasoning { text, .. } => {
                total += tools::estimate_text_tokens(text) as u64;
            }
            ContentBlock::ToolCall {
                name, arguments, ..
            } => {
                total += tools::estimate_text_tokens(name) as u64;
                total += tools::estimate_text_tokens(arguments) as u64;
            }
            ContentBlock::ResultCard {
                task_id,
                title,
                version,
            } => {
                total += tools::estimate_text_tokens(&ContentBlock::result_card_text(
                    task_id, title, *version,
                )) as u64;
            }
            ContentBlock::File { name, path, .. } => {
                total += tools::estimate_text_tokens(name) as u64;
                total += path
                    .as_deref()
                    .map_or(0, |path| tools::estimate_text_tokens(path) as u64);
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
    host.tools().estimate_schema_tokens()
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
    // DSH：溢出恢复的区间选择用 retain 0（强制一段有用的删除），压力路径用 policy 的保留预算。
    let retain_tokens = if force { 0 } else { budget.retain_tokens };
    let attempts = if force {
        1
    } else {
        (policy.compaction_retries as usize).saturating_add(1)
    };
    let mut outcome = None;
    for _ in 0..attempts {
        let messages = derive_messages(host);
        let Some(pre_split) = select_split(&messages, retain_tokens) else {
            if outcome.is_some() {
                return Err(AgentError::Compaction(
                    "no safe compaction range can reduce context below threshold".to_owned(),
                ));
            }
            return Ok(None);
        };
        // 压缩前记一笔：每个真实压缩事务一次；失败/超时只记一行，压缩照常。
        if let (Some(hook), Some(instruction)) =
            (host.pre_compaction(), host.pre_compaction_instruction())
        {
            let conversation = host
                .session()
                .lock()
                .expect("agent 会话锁被毒化")
                .id()
                .clone();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |duration| duration.subsec_nanos());
            let compaction_id = format!("pre-compact-{turn}-{step}-{nanos}");
            match pre_compaction_diary(host, &messages[..pre_split], instruction).await {
                Ok(text) if !text.trim().is_empty() => {
                    let diary = crate::PreCompactionDiary {
                        compaction_id,
                        conversation,
                        text,
                    };
                    match tokio::time::timeout(std::time::Duration::from_secs(30), hook(diary))
                        .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => crate::warn::warn(format!("压缩前记一笔失败：{error}")),
                        Err(_) => crate::warn::warn("压缩前记一笔超时"),
                    }
                }
                Ok(_) => {}
                Err(error) => crate::warn::warn(format!("压缩前记一笔请求失败：{error}")),
            }
        }
        // 只在 select_split 成功（真的有一个事务要提交）之后派发 Compaction 挂点：
        // prune 必须只在真实压缩事务里跑；每次真实事务派发一次（重试事务也算）。
        if notify_compaction {
            host.dispatch_compaction(turn, step, trigger).await?;
        }
        // prune 挂点可能已经把上下文压到阈值以下（DSH 在 pressure 路径剪枝后重测）：
        // 那么这次就不摘要，修剪本身已经落日志。溢出路径仍强制走一次压缩。
        if !force && measure(host) < budget.threshold_tokens {
            return Ok(outcome);
        }
        // prune 只换 tool/result 内容、不改派生节点数；按修剪后的内容重新选区间。
        let messages = derive_messages(host);
        let split = select_split(&messages, retain_tokens).unwrap_or(pre_split);
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
        .await
        .map_err(|error| {
            manual_error(
                CompactionTrigger::Manual,
                ManualCompactionErrorCode::Commit,
                error,
            )
        })?;
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
        return Err(manual_error(
            trigger,
            ManualCompactionErrorCode::Commit,
            AgentError::Compaction("invalid compaction split".to_owned()),
        ));
    }
    let shadowed_range = ShadowedRange {
        start: derived[1].0,
        end: derived[split - 1].0,
    };
    let shadowed_seqs: Vec<u64> = derived[1..split].iter().map(|(seq, _)| *seq).collect();
    let prefix: Vec<Message> = selected[..split].to_vec();
    let (outcome, _checkpoint_seq) = compact_transaction(
        host,
        turn,
        source_command_id,
        trigger,
        shadowed_range,
        shadowed_seqs,
        prefix,
        Some(1),
        policy,
    )
    .await?;
    Ok(outcome)
}

/// 一次可指定区间的 checkpoint 事务（T095.4：压缩/两种总结共用）。
///
/// 语义与旧 compact_once 相同：start → 摘要 → summary → 替换消息 → end。summary_input
/// 是投给摘要请求的消息；shadowed_seqs 是这次要替换掉的派生节点。expected_start_index
/// 为 Some 时按旧压缩的 changed 复核（替换范围必须还在原位置）；None 时只要求这些节点还在。
#[expect(
    clippy::too_many_arguments,
    reason = "一次事务的上下文、区间、摘要输入和策略都从调用处传进来"
)]
async fn compact_transaction(
    host: &dyn LoopAccess,
    turn: Option<u64>,
    source_command_id: Option<String>,
    trigger: CompactionTrigger,
    shadowed_range: ShadowedRange,
    shadowed_seqs: Vec<u64>,
    summary_input: Vec<Message>,
    expected_start_index: Option<usize>,
    policy: &CompactionPolicy,
) -> Result<(CompactionOutcome, u64), AgentError> {
    if shadowed_seqs.is_empty() {
        return Err(manual_error(
            trigger,
            ManualCompactionErrorCode::Commit,
            AgentError::Compaction("empty summary range".to_owned()),
        ));
    }
    let shadowed_token_count = estimate_messages(&summary_input);
    let fixed_blocks = host.compaction_fixed_blocks();

    // id 带进程/纳秒成分：续接或新进程都不会和旧 id 撞车。
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

    #[expect(
        clippy::let_underscore_must_use,
        reason = "页面未订阅时丢弃展示事件；持久化事实照常落盘"
    )]
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
        .await
        .map_err(|error| manual_error(trigger, ManualCompactionErrorCode::Persistence, error))?;
    let start_seq = start_seqs[0];

    let summary = match summarize(
        host,
        &summary_input,
        policy.summary_max_tokens,
        if host.compaction_profile() == crate::CompactionProfile::Kind {
            KIND_COMPACTION_INSTRUCTION
        } else {
            COMPACTION_INSTRUCTION
        },
    )
    .await
    {
        Ok(summary) => summary,
        Err(error) => {
            let message = error.to_string();
            #[expect(
                clippy::let_underscore_must_use,
                reason = "压缩已失败，补记 compaction/end 失败不覆盖原错误"
            )]
            let _ = host
                .append(&[EventKind::CompactionEnd {
                    compaction_id: compaction_id.clone(),
                    source_command_id: source_command_id.clone(),
                    turn,
                    error: Some(message.clone()),
                }])
                .await;
            #[expect(
                clippy::let_underscore_must_use,
                reason = "页面未订阅时丢弃展示事件；持久化事实照常落盘"
            )]
            let _ = host.events().send(AgentEvent::CompactionEnded {
                compaction_id,
                error: Some(message),
            });
            return Err(manual_error(
                trigger,
                ManualCompactionErrorCode::Summary,
                error,
            ));
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
        .await
        .map_err(|error| manual_error(trigger, ManualCompactionErrorCode::Persistence, error))?;
    let summary_seq = summary_seqs[0];

    // DSH manual compaction 的 changed 复核（command-compact expectedFailure 的 changed）：
    // 摘要请求期间历史可能被新消息改变；替换区间对不上就放弃替换、写 compaction/end(error)。
    {
        let current: Vec<u64> = {
            let store = host.session().lock().expect("agent 会话锁被毒化");
            store
                .derive_messages_with_seqs()
                .iter()
                .map(|(seq, _)| *seq)
                .collect()
        };
        let unchanged = match expected_start_index {
            Some(index) => {
                current.get(index..index + shadowed_seqs.len()) == Some(shadowed_seqs.as_slice())
            }
            None => shadowed_seqs.iter().all(|seq| current.contains(seq)),
        };
        if !unchanged {
            let message = "the history selected for compaction changed before it could be replaced"
                .to_owned();
            #[expect(
                clippy::let_underscore_must_use,
                reason = "压缩已失败，补记 compaction/end 失败不覆盖原错误"
            )]
            let _ = host
                .append(&[EventKind::CompactionEnd {
                    compaction_id: compaction_id.clone(),
                    source_command_id: source_command_id.clone(),
                    turn,
                    error: Some(message.clone()),
                }])
                .await;
            #[expect(
                clippy::let_underscore_must_use,
                reason = "页面未订阅时丢弃展示事件；持久化事实照常落盘"
            )]
            let _ = host.events().send(AgentEvent::CompactionEnded {
                compaction_id,
                error: Some(message.clone()),
            });
            return Err(manual_error(
                trigger,
                ManualCompactionErrorCode::Changed,
                AgentError::Compaction(message),
            ));
        }
    }

    // 替换写成日志里的 user/message：内容 = checkpoint 框架 + 摘要块；envelope 带
    // surfaceOp replace 与 sourceEventSeqs（start、summary、被替换节点）。
    let checkpoint = checkpoint_message(
        &compaction_id,
        source_command_id.as_deref(),
        &summary_blocks,
        &fixed_blocks,
        host.compaction_profile(),
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
    .await
    .map_err(|error| manual_error(trigger, ManualCompactionErrorCode::Commit, error))?;
    let checkpoint_seq = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        store.events().last().map(|event| event.seq).unwrap_or(0)
    };

    host.append(&[EventKind::CompactionEnd {
        compaction_id: compaction_id.clone(),
        source_command_id,
        turn,
        error: None,
    }])
    .await
    .map_err(|error| manual_error(trigger, ManualCompactionErrorCode::Persistence, error))?;
    #[expect(
        clippy::let_underscore_must_use,
        reason = "页面未订阅时丢弃展示事件；持久化事实照常落盘"
    )]
    let _ = host.events().send(AgentEvent::CompactionEnded {
        compaction_id,
        error: None,
    });
    Ok((
        CompactionOutcome {
            shadowed_items: shadowed_seqs.len(),
            shadowed_tokens: shadowed_token_count,
        },
        checkpoint_seq,
    ))
}

/// 恢复对话（T095.4）：写 conversation/rewound 事件，外壳 replace 圈住 from_seq 到当前日志末尾。
pub(crate) async fn rewind_restore(
    host: &dyn LoopAccess,
    from_seq: u64,
) -> Result<crate::RewindOutcome, AgentError> {
    let last_seq = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        store.events().last().map(|event| event.seq).unwrap_or(0)
    };
    if from_seq == 0 || from_seq > last_seq {
        return Err(AgentError::Compaction(format!(
            "rewind target {from_seq} is not in the session"
        )));
    }
    host.append_surface(
        EventKind::ConversationRewound {},
        SurfaceOp::Replace {
            start_seq: from_seq,
            end_seq: last_seq,
        },
        vec![from_seq, last_seq],
    )
    .await
    .map_err(|error| {
        manual_error(
            CompactionTrigger::Manual,
            ManualCompactionErrorCode::Persistence,
            error,
        )
    })?;
    let event_seq = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        store
            .events()
            .last()
            .map(|event| event.seq)
            .unwrap_or(last_seq)
    };
    Ok(crate::RewindOutcome {
        event_seq,
        checkpoint_seq: None,
        start_seq: from_seq,
        end_seq: last_seq,
        shadowed_items: 1,
        shadowed_tokens: 0,
    })
}

fn visible_derived(host: &dyn LoopAccess) -> Vec<(u64, Message)> {
    let store = host.session().lock().expect("agent 会话锁被毒化");
    store.derive_messages_with_seqs()
}

/// 从这里起总结（T095.4）：from_seq 到末尾压成一段。
pub(crate) async fn rewind_summarize_from(
    host: &dyn LoopAccess,
    from_seq: u64,
) -> Result<crate::RewindOutcome, AgentError> {
    let derived = visible_derived(host);
    let index = derived
        .iter()
        .position(|(seq, _)| *seq == from_seq)
        .ok_or_else(|| {
            AgentError::Compaction(format!("rewind target {from_seq} is not visible"))
        })?;
    let seqs: Vec<u64> = derived[index..].iter().map(|(seq, _)| *seq).collect();
    let messages: Vec<Message> = derived[index..]
        .iter()
        .map(|(_, message)| message.clone())
        .collect();
    let end_seq = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        store.events().last().map(|event| event.seq).unwrap_or(0)
    };
    let range = ShadowedRange {
        start: seqs[0],
        end: end_seq.max(seqs[0]),
    };
    let (outcome, checkpoint_seq) = compact_transaction(
        host,
        None,
        Some(format!("rewind:from:{from_seq}")),
        CompactionTrigger::Manual,
        range,
        seqs,
        messages,
        None,
        &host.compaction_policy(),
    )
    .await?;
    Ok(crate::RewindOutcome {
        event_seq: checkpoint_seq,
        checkpoint_seq: Some(checkpoint_seq),
        start_seq: range.start,
        end_seq: range.end,
        shadowed_items: outcome.shadowed_items,
        shadowed_tokens: outcome.shadowed_tokens,
    })
}

/// 总结到这里为止（T095.4）：上次 checkpoint 之后到 to_seq 之前压成一段。
pub(crate) async fn rewind_summarize_up_to(
    host: &dyn LoopAccess,
    to_seq: u64,
) -> Result<crate::RewindOutcome, AgentError> {
    let derived = visible_derived(host);
    let target = derived
        .iter()
        .position(|(seq, _)| *seq == to_seq)
        .ok_or_else(|| AgentError::Compaction(format!("rewind target {to_seq} is not visible")))?;
    let boundary = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        store
            .events()
            .iter()
            .filter(|event| event.seq < to_seq)
            .filter_map(|event| match &event.kind {
                EventKind::CompactionEnd { error: None, .. } => Some(event.seq),
                _ => None,
            })
            .max()
            .unwrap_or(0)
    };
    // 系统消息和上一份 checkpoint（压缩产物）都不进总结区间：checkpoint 不是被总结的内容。
    let start = derived
        .iter()
        .position(|(seq, message)| {
            *seq > boundary
                && message.role != Role::System
                && !matches!(message.source, MessageSource::CompactCheckpoint { .. })
        })
        .unwrap_or(derived.len());
    if start >= target {
        return Err(AgentError::Compaction(
            "no messages to summarize before the target".to_owned(),
        ));
    }
    let seqs: Vec<u64> = derived[start..target].iter().map(|(seq, _)| *seq).collect();
    let messages: Vec<Message> = derived[start..target]
        .iter()
        .map(|(_, message)| message.clone())
        .collect();
    let range = ShadowedRange {
        start: seqs[0],
        end: *seqs.last().expect("summary range is not empty"),
    };
    let (outcome, checkpoint_seq) = compact_transaction(
        host,
        None,
        Some(format!("rewind:up-to:{to_seq}")),
        CompactionTrigger::Manual,
        range,
        seqs,
        messages,
        None,
        &host.compaction_policy(),
    )
    .await?;
    Ok(crate::RewindOutcome {
        event_seq: checkpoint_seq,
        checkpoint_seq: Some(checkpoint_seq),
        start_seq: range.start,
        end_seq: range.end,
        shadowed_items: outcome.shadowed_items,
        shadowed_tokens: outcome.shadowed_tokens,
    })
}

/// checkpoint user 消息：DSH frameSummary 的三段（preamble+open tag / 摘要块 / close tag），
/// source 是 compact-checkpoint（与 compaction id、发起命令关联）。
fn checkpoint_message(
    compaction_id: &str,
    source_command_id: Option<&str>,
    summary: &[ContentBlock],
    fixed_blocks: &[crate::CompactionFixedBlock],
    profile: crate::CompactionProfile,
) -> Message {
    let mut content = vec![ContentBlock::text(COMPACTION_PREAMBLE)];
    if !fixed_blocks.is_empty() {
        let mut text = String::from("以下是程序重带的固定块原文，不是新任务：");
        for block in fixed_blocks {
            text.push_str(&format!("\n\n[{}]\n{}", block.kind, block.text));
        }
        content.push(ContentBlock::text(text));
    }
    content.push(ContentBlock::text(SUMMARY_OPEN_TAG));
    content.extend(summary.iter().cloned());
    content.push(ContentBlock::text(SUMMARY_CLOSE_TAG));
    if profile == crate::CompactionProfile::Kind {
        content.push(ContentBlock::text(
            "摘要里只有 id 的东西，用工具按 id 重新读，不要凭摘要还原。",
        ));
    }
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
async fn pre_compaction_diary(
    host: &dyn LoopAccess,
    prefix: &[Message],
    instruction: &str,
) -> Result<String, AgentError> {
    let mut input = crate::tool_pairing::sanitize(prefix.to_vec());
    input.push(Message::user_text(instruction));
    let (tools, history) = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        let derived = store.derive_messages_with_seqs();
        crate::tool_gate::request_tools(
            host.tools(),
            store.events(),
            &derived,
            &host.tool_keys(),
            host.model().config().tool_update,
        )
    };
    let tool_request = model::ToolRequest { tools, history };
    let cancel = model::CancelToken::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    let diary_model = host.model().with_effort(host.model().default_effort());
    let mut stream = diary_model.stream(&input, &tool_request, &cancel).await?;
    let mut text = String::new();
    while let Some(chunk) = stream.next().await {
        match chunk {
            Ok(StreamChunk::TextDelta { text: delta, .. }) => text.push_str(&delta),
            Ok(StreamChunk::Usage { .. })
            | Ok(StreamChunk::Finish { .. })
            | Ok(StreamChunk::BlockStart { .. })
            | Ok(StreamChunk::ReasoningDelta { .. })
            | Ok(StreamChunk::ToolCallDelta { .. })
            | Ok(StreamChunk::BlockEnd { .. }) => {}
            Err(error) => return Err(AgentError::Model(error)),
        }
    }
    Ok(text.trim().to_owned())
}

async fn summarize(
    host: &dyn LoopAccess,
    prefix: &[Message],
    max_tokens: u32,
    instruction: &str,
) -> Result<SummaryOutcome, AgentError> {
    let mut input = crate::tool_pairing::sanitize(prefix.to_vec());
    input.push(Message::user_text(instruction));
    // 摘要请求与主请求用同一份工具表/投影：没开出的工具不能进摘要请求。
    let (tools, history) = {
        let store = host.session().lock().expect("agent 会话锁被毒化");
        let derived = store.derive_messages_with_seqs();
        crate::tool_gate::request_tools(
            host.tools(),
            store.events(),
            &derived,
            &host.tool_keys(),
            host.model().config().tool_update,
        )
    };
    let tool_request = model::ToolRequest { tools, history };
    let cancel = model::CancelToken::new(
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        std::sync::Arc::new(tokio::sync::Notify::new()),
    );
    // 摘要请求用这个模型的 DefaultEffort（生产调用点，不再用用户/CLI 档）。
    let summary_model = host.model().with_effort(host.model().default_effort());
    let mut stream = summary_model
        .stream_with_max_tokens(&input, &tool_request, &cancel, Some(max_tokens))
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
            | Ok(StreamChunk::ToolCallDelta { .. })
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
    /// 本 crate 单测的凭证来源（产品不再有固定 key 构造入口）。
    struct TestCredential;

    impl model::CredentialSource for TestCredential {
        fn issue(
            &self,
            _purpose: &str,
            _context: &model::CredentialContext,
        ) -> Result<String, model::ModelError> {
            Ok("test-key".to_owned())
        }
    }

    fn test_credential() -> std::sync::Arc<dyn model::CredentialSource> {
        std::sync::Arc::new(TestCredential)
    }

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

    #[test]
    fn checkpoint_puts_fixed_blocks_before_summary_and_hints_once_for_kind() {
        let blocks = vec![
            crate::CompactionFixedBlock {
                kind: "instructions".to_owned(),
                identity: "i1".to_owned(),
                text: "工作区指令原文".to_owned(),
            },
            crate::CompactionFixedBlock {
                kind: "task".to_owned(),
                identity: "t1".to_owned(),
                text: "任务书原文".to_owned(),
            },
        ];
        let summary = vec![ContentBlock::text("摘要正文")];
        let message = checkpoint_message(
            "compaction-test",
            None,
            &summary,
            &blocks,
            crate::CompactionProfile::Kind,
        );
        let text = message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        let instruction_at = text.find("工作区指令原文").expect("指令在");
        let task_at = text.find("任务书原文").expect("任务书在");
        let summary_at = text.find("摘要正文").expect("摘要在");
        assert!(instruction_at < summary_at, "指令在摘要前");
        assert!(task_at < summary_at, "任务书在摘要前");
        assert_eq!(
            text.matches("摘要里只有 id 的东西").count(),
            1,
            "Kind id 提示只一次"
        );
        let thin = checkpoint_message(
            "compaction-thin",
            None,
            &summary,
            &blocks,
            crate::CompactionProfile::Thin,
        );
        let thin_text = thin
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            !thin_text.contains("摘要里只有 id 的东西"),
            "ThinAgent 不收 id 提示"
        );
        for section in [
            "在办的事和对谁的承诺",
            "在等什么、等谁",
            "任务 id 和状态",
            "最近用户原话",
            "已定下的约定",
            "产出文件",
            "下一步",
        ] {
            assert!(
                KIND_COMPACTION_INSTRUCTION.contains(section),
                "缺 {section}"
            );
        }
        for template in [KIND_COMPACTION_INSTRUCTION, COMPACTION_INSTRUCTION] {
            assert!(template.contains("可见 id"), "模板要给可见 id：{template}");
            assert!(template.contains("messageId"), "模板要写 messageId");
            assert!(!template.contains("seq"), "模板不能用 seq");
        }
    }

    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    use tokio::sync::broadcast;

    use crate::AgentEvent;
    use crate::capabilities::{BoxedFuture, HookFuture, HookOutcome};

    /// 只给压缩单测用的 LoopAccess 替身：记录 Compaction 挂点派发次数，模型指向
    /// 一个必然连不上的地址（摘要请求会快速失败，不进网络等待）。
    struct RecordingHost {
        store: Mutex<session::SessionStore>,
        events: broadcast::Sender<AgentEvent>,
        tools: tools::ToolSet,
        model: model::DeepSeek,
        tool_session: Arc<tools::ToolSessionState>,
        data_dir: PathBuf,
        policy: CompactionPolicy,
        compaction_dispatches: AtomicU32,
        _data: tempfile::TempDir,
        _cwd: tempfile::TempDir,
    }

    fn recording_host(policy: CompactionPolicy) -> RecordingHost {
        let data = tempfile::tempdir().unwrap();
        let cwd = tempfile::tempdir().unwrap();
        let store = session::SessionStore::create(data.path(), cwd.path()).unwrap();
        let (events, _receiver) = broadcast::channel(16);
        let model = model::DeepSeek::with_credentials(
            model::DeepSeekConfig {
                provider: "deepseek".to_owned(),
                base_url: "http://127.0.0.1:1".to_owned(),
                model: "deepseek-flash".to_owned(),
                max_tokens: 16,
                context_window: 1000,
                effort: model::Effort::new("high"),
                tool_update: None,
            },
            test_credential(),
        )
        .unwrap();
        RecordingHost {
            store: Mutex::new(store),
            events,
            tools: tools::ToolSet::empty(),
            model,
            tool_session: Arc::new(tools::ToolSessionState::new(data.path(), "compaction-test")),
            data_dir: data.path().to_path_buf(),
            policy,
            compaction_dispatches: AtomicU32::new(0),
            _data: data,
            _cwd: cwd,
        }
    }

    impl LoopAccess for RecordingHost {
        fn conversation_id(&self) -> &session::SessionId {
            unimplemented!()
        }
        fn session(&self) -> &Mutex<session::SessionStore> {
            &self.store
        }
        fn events(&self) -> &broadcast::Sender<AgentEvent> {
            &self.events
        }
        fn tools(&self) -> &tools::ToolSet {
            &self.tools
        }
        fn tool_session(&self) -> &Arc<tools::ToolSessionState> {
            &self.tool_session
        }
        fn model(&self) -> &model::DeepSeek {
            &self.model
        }
        fn cwd(&self) -> &Path {
            Path::new("/tmp/aios-test-cwd")
        }
        fn data_dir(&self) -> &Path {
            &self.data_dir
        }
        fn tool_keys(&self) -> std::collections::BTreeSet<tools::ToolKey> {
            std::collections::BTreeSet::new()
        }
        fn compaction_policy(&self) -> CompactionPolicy {
            self.policy
        }
        fn append<'a>(&'a self, kinds: &'a [EventKind]) -> HookFuture<'a> {
            Box::pin(async move {
                self.store
                    .lock()
                    .expect("测试 host 会话锁被毒化")
                    .append(kinds)?;
                Ok(HookOutcome::Continue)
            })
        }
        fn append_with_seqs<'a>(&'a self, kinds: &'a [EventKind]) -> BoxedFuture<'a, Vec<u64>> {
            Box::pin(async move {
                Ok(self
                    .store
                    .lock()
                    .expect("测试 host 会话锁被毒化")
                    .append(kinds)?)
            })
        }
        fn append_surface<'a>(
            &'a self,
            kind: EventKind,
            surface_op: SurfaceOp,
            source_event_seqs: Vec<u64>,
        ) -> HookFuture<'a> {
            Box::pin(async move {
                self.store
                    .lock()
                    .expect("测试 host 会话锁被毒化")
                    .append_surface(kind, surface_op, source_event_seqs)?;
                Ok(HookOutcome::Continue)
            })
        }
        fn dispatch_compaction<'a>(
            &'a self,
            _turn: u64,
            _step: u64,
            _trigger: CompactionTrigger,
        ) -> HookFuture<'a> {
            self.compaction_dispatches.fetch_add(1, Ordering::SeqCst);
            Box::pin(async move { Ok(HookOutcome::Continue) })
        }
        fn current_abort(&self) -> Option<Arc<AtomicBool>> {
            None
        }
        fn enqueue_context<'a>(&'a self, _text: String) -> BoxedFuture<'a, ()> {
            Box::pin(async move { Ok(()) })
        }
        fn enqueue_message<'a>(&'a self, _message: model::Message) -> BoxedFuture<'a, bool> {
            Box::pin(async move { Ok(true) })
        }
        fn is_running(&self) -> bool {
            false
        }
        fn last_turn(&self) -> u64 {
            0
        }
    }

    fn test_policy() -> CompactionPolicy {
        CompactionPolicy {
            // 阈值压到 1 token：只要 measure 有任何内容就算「越过阈值」。
            threshold_ratio: 0.000001,
            retain_ratio: 0.000001,
            headroom_tokens: 1,
            summary_max_tokens: 16,
            compaction_retries: 0,
            max_overflow_retries: 0,
        }
    }

    /// 阈值过但没有可压缩前缀（messages<3）：不能派发 Compaction 挂点（prune
    /// 只能跑在真实压缩事务里），也不写 compaction/* 事件。
    #[tokio::test]
    async fn threshold_over_without_compactable_prefix_does_not_dispatch_compaction() {
        let host = recording_host(test_policy());
        {
            let mut store = host.store.lock().expect("测试 host 会话锁被毒化");
            store
                .append(&[EventKind::SystemMessage {
                    turn: 1,
                    step: 1,
                    message: Message::system_prompt("only system prompt"),
                }])
                .unwrap();
        }
        let outcome = compact_if_needed(&host, 1, 1, CompactionTrigger::Auto, false, true)
            .await
            .expect("没有可压前缀不是错误");
        assert!(outcome.is_none(), "没有可压缩历史");
        assert_eq!(
            host.compaction_dispatches.load(Ordering::SeqCst),
            0,
            "没有真实事务就不能派发 Compaction 挂点"
        );
        let store = host.store.lock().expect("测试 host 会话锁被毒化");
        assert!(
            store
                .events()
                .iter()
                .all(|event| !event.kind.type_name().starts_with("compaction/")),
            "没有真实事务就不写 compaction 事件"
        );
    }

    /// 真的选出可压前缀后，每个真实事务派发一次 Compaction 挂点（本测试用必然失败的
    /// 摘要请求，验证派发发生在事务提交前、且只发生一次）。
    #[tokio::test]
    async fn real_compaction_transaction_dispatches_hook_once() {
        let host = recording_host(test_policy());
        {
            let mut store = host.store.lock().expect("测试 host 会话锁被毒化");
            store
                .append(&[
                    EventKind::TurnStart { turn: 1 },
                    EventKind::SystemMessage {
                        turn: 1,
                        step: 1,
                        message: Message::system_prompt("system prompt"),
                    },
                    EventKind::UserMessage {
                        turn: 1,
                        step: 1,
                        message: Message::user_text("a".repeat(400)),
                    },
                    EventKind::AssistantMessage {
                        effort: None,
                        effort_wanted: None,
                        turn: 1,
                        step: 1,
                        message: Message::assistant(
                            vec![ContentBlock::text("answer")],
                            "deepseek",
                            "deepseek-flash",
                        ),
                        stream: Vec::new(),
                        usage: None,
                        interrupted: None,

                        request_at: None,
                        context_parts: None,
                    },
                ])
                .unwrap();
        }
        let result = compact_if_needed(&host, 1, 1, CompactionTrigger::Auto, false, true).await;
        assert!(result.is_err(), "摘要请求连不上应失败");
        assert_eq!(
            host.compaction_dispatches.load(Ordering::SeqCst),
            1,
            "一个真实事务派发一次 Compaction 挂点"
        );
        let store = host.store.lock().expect("测试 host 会话锁被毒化");
        assert!(
            store
                .events()
                .iter()
                .any(|event| matches!(event.kind, EventKind::CompactionEnd { error: Some(_), .. })),
            "事务失败也写 compaction/end（带 error）"
        );
    }
}
