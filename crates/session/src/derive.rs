//! 从会话日志派生一次请求要带的消息。

use std::collections::HashMap;

use model::{ContentBlock, Message, Role};

use crate::{Event, EventKind};

/// 压缩 checkpoint 的开头（照 DSH compaction-basic 的 checkpoint preamble）。
pub const COMPACTION_PREAMBLE: &str = "This is an automatically generated checkpoint condensing an earlier span of the conversation to free up context. Treat the captured context as established background and build on it without restating it. Continue the task directly from the messages that follow, without acknowledging this checkpoint.";
pub const SUMMARY_OPEN_TAG: &str = "<compacted-summary>";
pub const SUMMARY_CLOSE_TAG: &str = "</compacted-summary>";

/// 按日志顺序派生消息；空消息跳过（照 DSH 的 deriveEventMessage）。
///
/// T005.2：compaction/prune 事件把某个 call_id 的 tool/result 换成修剪视图。
/// T005.8a：compaction/summary 事件把 shadowed 区间内的事件从派生视图里藏掉，
/// 并在区间起点插一条 checkpoint 用户消息；同一条日志只保留最后一次压缩的 checkpoint。
pub(crate) fn derive_messages(events: &[Event]) -> Vec<Message> {
    derive_messages_with_seqs(events)
        .into_iter()
        .map(|(_, message)| message)
        .collect()
}

/// 派生消息并带上来源事件 seq（T005.8a 压缩选区间用；checkpoint 用压缩 start 的 seq）。
pub(crate) fn derive_messages_with_seqs(events: &[Event]) -> Vec<(u64, Message)> {
    let mut replacements: HashMap<&str, &str> = HashMap::new();
    let mut compactions: Vec<(String, u64, u64, String)> = Vec::new();
    for event in events {
        match &event.kind {
            EventKind::CompactionPrune { call_id, content } => {
                replacements.insert(call_id.as_str(), content.as_str());
            }
            EventKind::CompactionSummary {
                compaction_id,
                summary,
                shadowed_start,
                shadowed_end,
                ..
            } => compactions.push((
                compaction_id.clone(),
                *shadowed_start,
                *shadowed_end,
                summary.clone(),
            )),
            _ => {}
        }
    }
    let last = compactions.last().cloned();
    let hidden = |seq: u64| {
        compactions
            .iter()
            .any(|(_, start, end, _)| seq >= *start && seq <= *end)
    };
    let mut derived: Vec<(u64, Message)> = Vec::new();
    for event in events {
        if let Some((compaction_id, start, _, summary)) = &last
            && event.seq == *start
        {
            let _ = compaction_id;
            let text = format!(
                "{COMPACTION_PREAMBLE}\n\n{SUMMARY_OPEN_TAG}\n{}\n{SUMMARY_CLOSE_TAG}",
                summary.trim()
            );
            derived.push((event.seq, Message::text(Role::User, text)));
            continue;
        }
        if hidden(event.seq) {
            continue;
        }
        let message = match &event.kind {
            EventKind::SystemMessage { message, .. }
            | EventKind::UserMessage { message, .. }
            | EventKind::AssistantMessage { message, .. } => message_or_skip(message),
            EventKind::ToolResult { message, .. } => {
                let Some(mut result_message) = message_or_skip(message) else {
                    continue;
                };
                for block in &mut result_message.content {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                        && let Some(replacement) = replacements.get(tool_use_id.as_str())
                    {
                        *content = (*replacement).to_owned();
                    }
                }
                Some(result_message)
            }
            EventKind::TurnStart { .. }
            | EventKind::TurnEnd { .. }
            | EventKind::StepStart { .. }
            | EventKind::StepEnd { .. }
            | EventKind::ToolCall { .. }
            | EventKind::CompactionPrune { .. }
            | EventKind::CompactionStart { .. }
            | EventKind::CompactionSummary { .. }
            | EventKind::CompactionEnd { .. }
            | EventKind::TodoWrite { .. }
            | EventKind::QuestionAsked { .. }
            | EventKind::QuestionCancelled { .. }
            | EventKind::InboxSpliced { .. } => None,
        };
        if let Some(message) = message {
            derived.push((event.seq, message));
        }
    }
    derived
}

fn message_or_skip(message: &Message) -> Option<Message> {
    if message.is_empty() {
        None
    } else {
        Some(message.clone())
    }
}
