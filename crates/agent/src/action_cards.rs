//! 确认卡决定的模型通知：用户点过按钮以后，下一次请求之前把还没报过的决定告诉模型。
//!
//! 通知是 system/message（开头照抄来源插件的原文）：决定是事实，不是让模型重新开始一件
//! 事的请求。报过的决定不再报；同一次请求里有几张卡的决定，合成一条通知。
//!
//! 只从会话日志推：卡片提议是 propose_action 的 tool/result（meta 带标题），决定是
//! card/decision 事件；通知自己的 seq 是「报过的界线」。

use model::{ContentBlock, Message, Role};
use session::{Event, EventKind};

use crate::capabilities::{
    Capability, HookEvent, HookFuture, HookOutcome, HookPoint, PreStepPhase,
};

/// 来源插件原文的开头，照抄不改。
pub(crate) const STATUS_PREAMBLE: &str = "Latest status of action cards from user actions and background work. These are status facts, not a request to start or retry work. Use these receipts instead of the earlier proposal when describing progress.";
/// 来源插件原文的结尾，照抄不改。
pub(crate) const STATUS_CLOSING: &str = "Continue with the user's current request.";

/// 每步注入一次状态通知（只有存在还没报过的决定时才追加消息）。
pub(crate) struct ActionCardCapability;

impl Capability for ActionCardCapability {
    fn name(&self) -> &'static str {
        "action-cards"
    }

    fn hooks(&self) -> &'static [HookPoint] {
        &[HookPoint::PreStep]
    }

    fn on_hook<'a>(&'a self, event: &'a HookEvent<'a>) -> HookFuture<'a> {
        Box::pin(async move {
            let HookEvent::PreStep {
                turn,
                step,
                phase,
                trigger,
                ..
            } = event
            else {
                return Ok(HookOutcome::Continue);
            };
            if *phase != PreStepPhase::Inject || trigger.is_some() {
                return Ok(HookOutcome::Continue);
            }
            let events = event
                .host()
                .session()
                .lock()
                .expect("agent 会话锁被毒化")
                .events()
                .to_vec();
            let Some(message) = status_notice(&events) else {
                return Ok(HookOutcome::Continue);
            };
            event
                .host()
                .append(&[EventKind::SystemMessage {
                    turn: *turn,
                    step: *step,
                    message,
                }])
                .await?;
            Ok(HookOutcome::Continue)
        })
    }
}

/// 还没报过的决定 → 一条 system 消息；没有决定或都报过了就是 None。
pub(crate) fn status_notice(events: &[Event]) -> Option<Message> {
    // 通知自己 seq 之前的决定都报过了；之后的还没报。
    let reported_through = events
        .iter()
        .filter(|event| {
            matches!(
                &event.kind,
                EventKind::SystemMessage { message, .. } if starts_with_preamble(message)
            )
        })
        .map(|event| event.seq)
        .max()
        .unwrap_or(0);
    // 同一张卡多次决定只报最新一次。
    let mut latest: std::collections::BTreeMap<String, (&Event, session::CardDecision)> =
        std::collections::BTreeMap::new();
    for event in events {
        if event.seq <= reported_through {
            continue;
        }
        let Some(decision) = session::CardDecision::from_event(&event.kind) else {
            continue;
        };
        latest.insert(decision.call_id.clone(), (event, decision));
    }
    if latest.is_empty() {
        return None;
    }
    let mut ordered: Vec<(&Event, &session::CardDecision)> = latest
        .values()
        .map(|(event, decision)| (*event, decision))
        .collect();
    ordered.sort_by_key(|(event, _)| event.seq);
    let updates: Vec<serde_json::Value> = ordered
        .into_iter()
        .map(|(_, decision)| {
            let mut update = serde_json::Map::new();
            update.insert("callId".to_owned(), serde_json::json!(decision.call_id));
            if let Some(title) = proposal_title(events, &decision.call_id) {
                update.insert("title".to_owned(), serde_json::json!(title));
            }
            if let Some(action) = &decision.action {
                update.insert("action".to_owned(), serde_json::json!(action));
            }
            update.insert(
                "phase".to_owned(),
                serde_json::json!(decision.phase.as_str()),
            );
            if let Some(data) = &decision.data {
                update.insert("data".to_owned(), data.clone());
            }
            serde_json::Value::Object(update)
        })
        .collect();
    let text = format!(
        "{STATUS_PREAMBLE} {STATUS_CLOSING}\n{}",
        serde_json::Value::Array(updates)
    );
    Some(Message::text(Role::System, text))
}

fn starts_with_preamble(message: &Message) -> bool {
    first_text(message).is_some_and(|text| text.starts_with(STATUS_PREAMBLE))
}

fn first_text(message: &Message) -> Option<&str> {
    message.content.iter().find_map(|block| match block {
        ContentBlock::Text { text, .. } => Some(text.as_str()),
        ContentBlock::Reasoning { .. }
        | ContentBlock::ToolCall { .. }
        | ContentBlock::File { .. }
        | ContentBlock::ResultCard { .. } => None,
    })
}

/// 提议卡片的标题：从 propose_action 的 tool/result meta 里取（没有就不写这一行）。
fn proposal_title(events: &[Event], call_id: &str) -> Option<String> {
    events.iter().find_map(|event| {
        let EventKind::ToolResult { message, meta, .. } = &event.kind else {
            return None;
        };
        if message.tool_call_id.as_deref() != Some(call_id) {
            return None;
        }
        meta.as_ref()?
            .get("card")?
            .get("title")?
            .as_str()
            .map(str::to_owned)
    })
}
