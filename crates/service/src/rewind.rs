//! T095.4：恢复/总结的服务面派生与校验（区间只认会话日志事实）。
//!
//! list：最后一次压缩（含两种总结）之后、当前可见的用户自己发的消息，最新在下。
//! 校验：跑着不归这里（handler 判）；这里判目标在最后一次压缩之后、是可见的用户消息。

use model::Message;
use session::{Event, EventKind, SurfaceOp};

use crate::error::ServiceError;
use crate::protocol::{RewindAction, RewindMessage};

/// 所有 surface replace 区间（压缩、总结、恢复都在里面）。
pub(crate) fn hidden_ranges(events: &[Event]) -> Vec<(u64, u64)> {
    events
        .iter()
        .filter_map(|event| match event.surface_op {
            Some(SurfaceOp::Replace { start_seq, end_seq }) => Some((start_seq, end_seq)),
            None => None,
        })
        .collect()
}

fn is_hidden(seq: u64, ranges: &[(u64, u64)]) -> bool {
    ranges
        .iter()
        .any(|(start, end)| seq >= *start && seq <= *end)
}

/// 最后一次压缩 checkpoint 的边界（总结复用同一 checkpoint；恢复不算 checkpoint）。
fn last_checkpoint_boundary(events: &[Event]) -> u64 {
    events
        .iter()
        .filter(|event| !matches!(event.kind, EventKind::ConversationRewound { .. }))
        .filter_map(|event| match event.surface_op {
            Some(SurfaceOp::Replace { end_seq, .. }) => Some(end_seq),
            None => None,
        })
        .max()
        .unwrap_or(0)
}

fn first_line(message: &Message) -> String {
    let text = message
        .content
        .iter()
        .filter_map(model::ContentBlock::as_text)
        .collect::<Vec<_>>()
        .join(" ");
    let line = text.lines().next().unwrap_or("").trim().to_owned();
    line
}

fn is_visible_user_message(event: &Event, hidden: &[(u64, u64)]) -> bool {
    if is_hidden(event.seq, hidden) {
        return false;
    }
    let EventKind::UserMessage { message, .. } = &event.kind else {
        return false;
    };
    matches!(&message.source, model::MessageSource::User { .. })
        && !crate::program_messages::is_program_message(message)
}

/// 菜单候选：最后一次 checkpoint 之后、可见、用户自己发的消息（最新在下）。
pub(crate) fn candidates(events: &[Event]) -> Vec<RewindMessage> {
    let hidden = hidden_ranges(events);
    let boundary = last_checkpoint_boundary(events);
    events
        .iter()
        .filter(|event| event.seq > boundary)
        .filter(|event| is_visible_user_message(event, &hidden))
        .filter_map(|event| {
            let EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let text = first_line(message);
            if text.is_empty() {
                return None;
            }
            Some(RewindMessage {
                seq: event.seq,
                time: event.time,
                text,
            })
        })
        .collect()
}

/// apply 的校验；不通过回可读原因（页面直接显示）。
pub(crate) fn validate(
    events: &[Event],
    from_seq: u64,
    action: RewindAction,
) -> Result<(), ServiceError> {
    let hidden = hidden_ranges(events);
    if is_hidden(from_seq, &hidden) {
        return Err(ServiceError::RewindRejected(
            "这句已经被恢复或压缩藏起来了，不能再选".to_owned(),
        ));
    }
    let boundary = last_checkpoint_boundary(events);
    if from_seq <= boundary {
        return Err(ServiceError::RewindRejected(
            "这句在最后一次压缩之前，不能再回退或总结".to_owned(),
        ));
    }
    let Some(event) = events.iter().find(|event| event.seq == from_seq) else {
        return Err(ServiceError::RewindRejected(
            "找不到这句，可能日志已经变了".to_owned(),
        ));
    };
    if !matches!(event.kind, EventKind::UserMessage { .. }) {
        return Err(ServiceError::RewindRejected(
            "只能从你说过的话开始".to_owned(),
        ));
    }
    if !is_visible_user_message(event, &hidden) {
        return Err(ServiceError::RewindRejected(
            "只能选你自己说的、没被程序或恢复藏掉的话".to_owned(),
        ));
    }
    if action == RewindAction::SummarizeUpTo {
        let has_earlier = events.iter().any(|candidate| {
            candidate.seq > boundary
                && candidate.seq < from_seq
                && !is_hidden(candidate.seq, &hidden)
                && matches!(
                    candidate.kind,
                    EventKind::SystemMessage { .. }
                        | EventKind::UserMessage { .. }
                        | EventKind::AssistantMessage { .. }
                        | EventKind::ToolResult { .. }
                )
        });
        if !has_earlier {
            return Err(ServiceError::RewindRejected(
                "这句前面没有可以总结的内容".to_owned(),
            ));
        }
    }
    Ok(())
}
