//! 从会话事件读断言材料：relay/cc/停止/重启/工具结果/卡片事实。

use super::*;

/// 从 Kind 请求体里「已建任务图 <id>」取出图 id（动态回放用；多个时取第一个）。
pub(super) fn graph_root_id(body: &str) -> Option<String> {
    let rest = body.split("已建任务图 ").nth(1)?;
    let id: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (!id.is_empty()).then_some(id)
}

/// 同一请求里出现多张图时取最后一个（第二轮那张图）。
pub(super) fn graph_root_id_last(body: &str) -> Option<String> {
    let rest = body.rsplit("已建任务图 ").next()?;
    let id: String = rest.chars().take_while(char::is_ascii_hexdigit).collect();
    (!id.is_empty()).then_some(id)
}

/// 从 ThinAgent 请求里第一条 assignment 取出任务 id 和标题；别的请求没有就是 None。
pub(super) fn assignment_identity(body: &str) -> Option<(String, String)> {
    let value: Value = serde_json::from_str(body).ok()?;
    for message in value["messages"].as_array()? {
        for block in message["content"].as_array()? {
            let Some(text) = block["text"].as_str() else {
                continue;
            };
            let Ok(assignment) = serde_json::from_str::<Value>(text) else {
                continue;
            };
            if let (Some(id), Some(title)) =
                (assignment["task_id"].as_str(), assignment["title"].as_str())
            {
                return Some((id.to_owned(), title.to_owned()));
            }
        }
    }
    None
}

pub(super) fn tool_names(body: &str) -> Vec<String> {
    let value: Value = serde_json::from_str(body).expect("请求体 JSON");
    value["tools"]
        .as_array()
        .expect("tools 数组")
        .iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_owned))
        .collect()
}

pub(super) fn relay_messages(events: &[session::Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            let rpc_id = rpc_id.as_deref()?;
            if !rpc_id.starts_with("task-report:") {
                return None;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            Some((rpc_id.to_owned(), serde_json::from_str(&text).ok()?))
        })
        .collect()
}

/// Kind 收到的抄送：rpc id 是 task-cc:<inputId>，正文是 task_cc JSON。
///
/// Inject 不唤醒时消息只落 agent/inbox/spliced（还没领取），被下一次请求领走后才
/// 变成 user/message；两处都收，按 rpc id 去重。
pub(super) fn cc_messages(events: &[session::Event]) -> Vec<(String, Value)> {
    let mut found: Vec<(String, Value)> = Vec::new();
    for event in events {
        let messages: Vec<&model::Message> = match &event.kind {
            session::EventKind::UserMessage { message, .. } => vec![message],
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().collect(),
            _ => continue,
        };
        for message in messages {
            let model::MessageSource::User { rpc_id } = &message.source else {
                continue;
            };
            let Some(rpc_id) = rpc_id.as_deref() else {
                continue;
            };
            if !rpc_id.starts_with("task-cc:") {
                continue;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            let Ok(body) = serde_json::from_str(&text) else {
                continue;
            };
            if let Some(entry) = found.iter_mut().find(|(id, _)| id == rpc_id) {
                entry.1 = body;
            } else {
                found.push((rpc_id.to_owned(), body));
            }
        }
    }
    found
}

/// Kind 收到的停止 relay：rpc id 是 task-stop:<taskId>:<attemptId>。
pub(super) fn stopped_relays(events: &[session::Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            let rpc_id = rpc_id.as_deref()?;
            if !rpc_id.starts_with("task-stop:") {
                return None;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            Some((rpc_id.to_owned(), serde_json::from_str(&text).ok()?))
        })
        .collect()
}

pub(super) fn last_turn_end(events: &[session::Event]) -> Option<session::TurnEndReason> {
    events.iter().rev().find_map(|event| match &event.kind {
        session::EventKind::TurnEnd { reason, .. } => Some(reason.clone()),
        _ => None,
    })
}

/// 这次 Attempt 的会话最后一件事是 turn/end：报告之后那段收尾也结束了，会话写者马上放开。
pub(super) fn session_turn_ended(data: &Path, attempt: &tasks::Attempt) -> bool {
    let Some(conversation) = attempt.conversation.clone() else {
        return false;
    };
    let Ok(id) = session::SessionId::parse(conversation.as_str()) else {
        return false;
    };
    let Ok(store) = session::SessionStore::open_read_only(data, &id) else {
        return false;
    };
    matches!(
        store.events().last().map(|event| &event.kind),
        Some(session::EventKind::TurnEnd { .. })
    )
}

/// 一个 Attempt 的 ThinAgent 会话日志（只读）。
pub(super) fn thin_events(data: &Path, attempt: &tasks::Attempt) -> Vec<session::Event> {
    let conversation = attempt.conversation.clone().expect("会话 id");
    let session_id = session::SessionId::parse(conversation.as_str()).expect("会话 id");
    session::SessionStore::open_read_only(data, &session_id)
        .expect("只读打开任务会话")
        .events()
        .to_vec()
}

/// 收尾停按 request id 幂等：同一个 id 的消息在会话日志里只该有一条。
pub(super) fn injected_stop_notes(
    events: &[session::Event],
    task: &Task,
    attempt: &tasks::Attempt,
) -> usize {
    let request_id = format!("task-stop:{}:{}", task.id, attempt.id);
    events
        .iter()
        .filter(|event| match &event.kind {
            session::EventKind::UserMessage { message, .. } => matches!(
                &message.source,
                model::MessageSource::User { rpc_id: Some(rpc) } if rpc == &request_id
            ),
            _ => false,
        })
        .count()
}

/// 一条 tool/result 的正文和 is_error。
pub(super) fn tool_result(events: &[session::Event], call_id: &str) -> (String, Option<bool>) {
    events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult { message, .. }
                if message.tool_call_id.as_deref() == Some(call_id) =>
            {
                Some((
                    message
                        .content
                        .iter()
                        .filter_map(model::ContentBlock::as_text)
                        .collect::<String>(),
                    message.is_error,
                ))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("{call_id} 的 tool/result"))
}

pub(super) fn waiting_relays(data: &Path) -> Vec<(String, Value)> {
    relay_messages(&crate::common::read_root_events(data))
}

/// 从提议工具结果的 meta 里取第一项的事实行。
pub(super) fn card_facts(events: &[session::Event], call_id: &str) -> Vec<Value> {
    let meta = events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                meta: Some(meta),
                ..
            } if message.tool_call_id.as_deref() == Some(call_id) => Some(meta.clone()),
            _ => None,
        })
        .expect("提议工具结果带 meta");
    meta["card"]["options"][0]["facts"]
        .as_array()
        .expect("事实数组")
        .clone()
}

/// Kind 收到的重启汇报：rpc id 是 restart:<bootId>，正文是 tasks_interrupted JSON。
pub(super) fn restart_messages(events: &[session::Event]) -> Vec<(String, Value)> {
    events
        .iter()
        .filter_map(|event| {
            let session::EventKind::UserMessage { message, .. } = &event.kind else {
                return None;
            };
            let model::MessageSource::User { rpc_id } = &message.source else {
                return None;
            };
            let rpc_id = rpc_id.as_deref()?;
            if !rpc_id.starts_with("restart:") {
                return None;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            Some((rpc_id.to_owned(), serde_json::from_str(&text).ok()?))
        })
        .collect()
}

/// 接续的第一条输入：rpc id 是 resume:<taskId>:<attemptId>。
///
/// 消息在日志里可能出现两次（inbox/spliced 插入 + 领取后的 user/message），按 rpc id 去重。
pub(super) fn recovery_notes(events: &[session::Event]) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    for event in events {
        let messages: Vec<&model::Message> = match &event.kind {
            session::EventKind::UserMessage { message, .. } => vec![message],
            session::EventKind::InboxSpliced { inserted, .. } => inserted.iter().collect(),
            _ => continue,
        };
        for message in messages {
            let model::MessageSource::User { rpc_id } = &message.source else {
                continue;
            };
            let Some(rpc_id) = rpc_id.as_deref() else {
                continue;
            };
            if !rpc_id.starts_with("resume:") {
                continue;
            }
            let text: String = message
                .content
                .iter()
                .filter_map(model::ContentBlock::as_text)
                .collect();
            if !found.iter().any(|(id, _)| id == rpc_id) {
                found.push((rpc_id.to_owned(), text));
            }
        }
    }
    found
}

/// 一张卡的全部选项（id/label/facts）。
pub(super) fn card_options(events: &[session::Event], call_id: &str) -> Vec<Value> {
    events
        .iter()
        .find_map(|event| match &event.kind {
            session::EventKind::ToolResult {
                message,
                meta: Some(meta),
                ..
            } if message.tool_call_id.as_deref() == Some(call_id) => Some(
                meta["card"]["options"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .unwrap_or_default()
}

/// 等第一条 ready relay，返回它的 request id 和正文。
pub(super) async fn ready_relay(fixture: &Fixture) -> (String, Value) {
    wait_until("ready relay", || {
        relay_messages(&crate::common::read_root_events(fixture.data()))
            .iter()
            .any(|(_, body)| body["state"] == "ready")
    })
    .await;
    relay_messages(&crate::common::read_root_events(fixture.data()))
        .into_iter()
        .find(|(_, body)| body["state"] == "ready")
        .expect("ready relay")
}

/// 取一条消息里的纯文本（工具结果正文）。
pub(super) fn message_text(message: &model::Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            model::ContentBlock::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}
