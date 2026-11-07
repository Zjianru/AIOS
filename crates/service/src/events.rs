//! 会话日志事件到服务事件的投影：失败原因、上下文占用和分页选择。

use crate::protocol::{FailureCause, ServiceEvent};

/// events 的分页选择（beforeSeq 不含本条；照 DSH history.ts:124-140）。
pub(crate) fn page_events(
    all: &[session::Event],
    before_seq: Option<u64>,
    max_messages: usize,
    min_messages: Option<usize>,
    min_turns: Option<usize>,
) -> (Vec<&session::Event>, bool) {
    if all.is_empty() {
        return (Vec::new(), false);
    }
    // beforeSeq 是「不含本条」的上界（照 DSH history.ts:124-140）；省略 = 日志末端之后。
    let exclusive_end = match before_seq {
        Some(seq) => seq,
        None => all.last().map(|event| event.seq + 1).unwrap_or(0),
    };
    if exclusive_end == 0 {
        return (Vec::new(), false);
    }
    let min_messages = min_messages.unwrap_or(50).min(max_messages);
    let min_turns = min_turns.unwrap_or(2).max(1);
    let mut selected: Vec<&session::Event> = Vec::new();
    let mut messages = 0_usize;
    let mut turns = 0_usize;
    for event in all.iter().filter(|event| event.seq < exclusive_end).rev() {
        selected.push(event);
        match &event.kind {
            session::EventKind::UserMessage { .. }
            | session::EventKind::AssistantMessage { .. } => messages += 1,
            session::EventKind::TurnStart { .. } => turns += 1,
            _ => {}
        }
        if messages >= max_messages {
            break;
        }
        if matches!(event.kind, session::EventKind::TurnStart { .. })
            && messages >= min_messages
            && turns >= min_turns
        {
            break;
        }
    }
    selected.reverse();
    let first_seq = selected.first().map(|event| event.seq).unwrap_or(0);
    let has_more = first_seq > all[0].seq;
    (selected, has_more)
}

/// 会话日志事件 → 服务事件（data = 日志事件的字段，不含 type 标签）。
pub(crate) fn service_event(event: &session::Event) -> ServiceEvent {
    let mut data = serde_json::to_value(&event.kind).unwrap_or(serde_json::Value::Null);
    if let Some(object) = data.as_object_mut() {
        object.remove("type");
    }
    ServiceEvent {
        seq: event.seq,
        time: event.time,
        event_type: event.kind.type_name().to_owned(),
        data,
        cause: failure_cause(&event.kind),
    }
}

/// 失败原因分类只从日志里的 turn/end error code 派生（页面不按 code 猜）。
pub(crate) fn failure_cause(kind: &session::EventKind) -> Option<FailureCause> {
    let session::EventKind::TurnEnd {
        reason: session::TurnEndReason::Error { error },
        ..
    } = kind
    else {
        return None;
    };
    Some(match error.code.as_str() {
        "KEYRING" => FailureCause::Keyring,
        // QUOTA 归凭证类（下一步是检查账户）。
        "AUTH" | "MISSING_CREDENTIAL" | "QUOTA" => FailureCause::Credential,
        _ => FailureCause::Model,
    })
}

/// 从一条服务事件里读上下文占用（assistant/message 的 usage 的 prompt 部分）。
pub(crate) fn context_used(event: &ServiceEvent) -> Option<u64> {
    if event.event_type != "assistant/message" {
        return None;
    }
    let usage = event.data.get("usage")?;
    let field = |name: &str| {
        usage
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Some(field("inputTokens") + field("cacheReadTokens") + field("cacheWriteTokens"))
}
