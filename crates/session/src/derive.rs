//! 从会话日志派生一次请求要带的消息。

use std::collections::HashMap;

use model::{ContentBlock, Message};

use crate::{Event, EventKind};

/// 按日志顺序派生消息；空消息跳过（照 DSH 的 deriveEventMessage）。
///
/// T005.2：compaction/prune 事件记录「某个 call_id 的 tool/result 在派生视图里
/// 应换成这段内容」；原始 tool/result 事件仍在日志里（可回放），这里只应用覆盖。
pub(crate) fn derive_messages(events: &[Event]) -> Vec<Message> {
    let mut replacements: HashMap<&str, &str> = HashMap::new();
    for event in events {
        if let EventKind::CompactionPrune { call_id, content } = &event.kind {
            replacements.insert(call_id.as_str(), content.as_str());
        }
    }
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::SystemMessage { message, .. }
            | EventKind::UserMessage { message, .. }
            | EventKind::AssistantMessage { message, .. } => message_or_skip(message),
            EventKind::ToolResult { message, .. } => {
                let mut message = message_or_skip(message)?;
                for block in &mut message.content {
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
                Some(message)
            }
            EventKind::TurnStart { .. }
            | EventKind::TurnEnd { .. }
            | EventKind::StepStart { .. }
            | EventKind::StepEnd { .. }
            | EventKind::ToolCall { .. }
            | EventKind::CompactionPrune { .. } => None,
        })
        .collect()
}

fn message_or_skip(message: &Message) -> Option<Message> {
    if message.is_empty() {
        None
    } else {
        Some(message.clone())
    }
}
