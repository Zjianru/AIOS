//! 从会话日志派生一次请求要带的消息。

use std::collections::HashMap;

use model::Message;

use crate::{Event, EventKind, SurfaceOp};

/// 按日志顺序派生消息；空消息跳过（照 DSH 的 deriveEventMessage）。
///
/// surface 模型照 DSH：带 `surfaceOp: replace` 的消息替换 [startSeq, endSeq] 区间里的
/// 派生节点（T006.3 压缩 checkpoint 用；T006.4 pruner 复用），替换消息本身照常派生。
/// compaction/prune 事件仍记录某个 call_id 的 tool/result 修剪视图（T005.2，T006.4 改形）。
pub(crate) fn derive_messages(events: &[Event]) -> Vec<Message> {
    derive_messages_with_seqs(events)
        .into_iter()
        .map(|(_, message)| message)
        .collect()
}

/// 派生消息并带上来源事件 seq（压缩选区间/替换消息定位用）。
pub(crate) fn derive_messages_with_seqs(events: &[Event]) -> Vec<(u64, Message)> {
    let mut replacements: HashMap<&str, &str> = HashMap::new();
    let mut shadowed: Vec<(u64, u64)> = Vec::new();
    for event in events {
        if let EventKind::CompactionPrune { call_id, content } = &event.kind {
            replacements.insert(call_id.as_str(), content.as_str());
        }
        if let Some(SurfaceOp::Replace { start_seq, end_seq }) = event.surface_op {
            shadowed.push((start_seq, end_seq));
        }
    }
    let hidden = |seq: u64| {
        shadowed
            .iter()
            .any(|(start, end)| seq >= *start && seq <= *end)
    };
    let mut derived: Vec<(u64, Message)> = Vec::new();
    for event in events {
        // 被更晚的 surface replace 藏掉的事件不进派生视图（替换消息自身 seq > endSeq，不会被藏）。
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
                // 修剪替换：按消息级 toolCallId 找到同一次调用，content 换成修剪视图。
                // 消息 id 不变（续接/派生看到同一个 id）。
                if let Some(call_id) = result_message.tool_call_id.clone()
                    && let Some(replacement) = replacements.get(call_id.as_str())
                {
                    result_message.content = vec![model::ContentBlock::text(*replacement)];
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
            | EventKind::AssistantAttempt { .. }
            | EventKind::LlmRetry { .. }
            | EventKind::LlmRetryStarted { .. }
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
