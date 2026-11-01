//! 从会话日志派生一次请求要带的消息。

use model::Message;

use crate::{Event, EventKind};

/// 按日志顺序派生消息；空消息跳过（照 DSH 的 deriveEventMessage）。
pub(crate) fn derive_messages(events: &[Event]) -> Vec<Message> {
    events
        .iter()
        .filter_map(|event| match &event.kind {
            EventKind::SystemMessage { message, .. }
            | EventKind::UserMessage { message, .. }
            | EventKind::AssistantMessage { message, .. }
            | EventKind::ToolResult { message, .. } => {
                if message.is_empty() {
                    None
                } else {
                    Some(message.clone())
                }
            }
            EventKind::TurnStart { .. }
            | EventKind::TurnEnd { .. }
            | EventKind::StepStart { .. }
            | EventKind::StepEnd { .. }
            | EventKind::ToolCall { .. } => None,
        })
        .collect()
}
