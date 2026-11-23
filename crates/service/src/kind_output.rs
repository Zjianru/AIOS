//! T078.1：Kind 的输出通道。kind_react 先回一个表情，kind_say 最后分条说；
//! 两个工具都只把调用/结果写进日志，不改 assistant 事件或日志类型。

use serde_json::{Value, json};
use tools::{Tool, ToolCall, ToolContext, ToolFailure, ToolFuture, ToolResult};

const EMOJI_IDS: [&str; 11] = [
    "+1",
    "ok_hand",
    "eyes",
    "pray",
    "heart",
    "tada",
    "cry",
    "sweat_smile",
    "saluting_face",
    "yawning_face",
    "joy",
];

/// 给用户本轮消息回一个表情。
pub struct KindReactTool {
    parameters: Value,
}

impl KindReactTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "emoji_id": {
                        "type": "string",
                        "enum": EMOJI_IDS,
                        "description": "One emoji id; do not pass arbitrary characters."
                    }
                },
                "required": ["emoji_id"]
            }),
        }
    }
}

impl Default for KindReactTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for KindReactTool {
    fn name(&self) -> &str {
        "kind_react"
    }

    fn description(&self) -> &str {
        "React to the user's latest message with one emoji before answering. Call at most once, before kind_say; only use when a human would react. An emoji never replaces the answer."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let args: Value = match serde_json::from_str(&call.arguments) {
                Ok(args) => args,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("arguments must be a JSON object: {error}")),
                    );
                }
            };
            let Some(emoji_id) = args.get("emoji_id").and_then(Value::as_str) else {
                return ToolResult::error(call.id, ToolFailure::plain("missing emoji_id"));
            };
            if !EMOJI_IDS.contains(&emoji_id) {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain(format!(
                        "unknown emoji_id {emoji_id:?}; use one of {EMOJI_IDS:?}"
                    )),
                );
            }
            let turn = context.turn();
            let user_message_seq =
                context
                    .session_events()
                    .iter()
                    .rev()
                    .find_map(|event| match &event.kind {
                        session::EventKind::UserMessage {
                            turn: event_turn,
                            message,
                            ..
                        } if *event_turn == turn
                            && !crate::program_messages::is_program_message(message) =>
                        {
                            Some(event.seq)
                        }
                        _ => None,
                    });
            let Some(user_message_seq) = user_message_seq else {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain("这一轮没有用户的消息可以回"),
                );
            };
            ToolResult::success_with_meta(
                call.id,
                format!("Reacted with {emoji_id}."),
                json!({
                    "kind_react": {
                        "emoji_id": emoji_id,
                        "user_message_seq": user_message_seq,
                    }
                }),
            )
        })
    }
}

/// 最后分条说：parts 至少两条，没有条数上限。
pub struct KindSayTool {
    parameters: Value,
}

impl KindSayTool {
    pub fn new() -> Self {
        Self {
            parameters: json!({
                "type": "object",
                "properties": {
                    "parts": {
                        "type": "array",
                        "minItems": 2,
                        "description": "Two or more message bubbles. Split at changes of thought; keep the same total length as one answer. Only use when the answer genuinely has multiple beats.",
                        "items": {"type": "string"}
                    }
                },
                "required": ["parts"]
            }),
        }
    }
}

impl Default for KindSayTool {
    fn default() -> Self {
        Self::new()
    }
}

impl Tool for KindSayTool {
    fn name(&self) -> &str {
        "kind_say"
    }

    fn description(&self) -> &str {
        "Say the final answer as two or more message bubbles. Call this last; after a successful call the turn ends without another model call. Split at changes of thought; do not use it for a one-bubble answer."
    }

    fn parameters(&self) -> &Value {
        &self.parameters
    }

    fn execute<'a>(
        &'a self,
        call: ToolCall,
        context: &'a ToolContext,
    ) -> ToolFuture<'a, ToolResult> {
        Box::pin(async move {
            let _ = context;
            let args: Value = match serde_json::from_str(&call.arguments) {
                Ok(args) => args,
                Err(error) => {
                    return ToolResult::error(
                        call.id,
                        ToolFailure::plain(format!("arguments must be a JSON object: {error}")),
                    );
                }
            };
            let Some(parts) = args.get("parts").and_then(Value::as_array) else {
                return ToolResult::error(call.id, ToolFailure::plain("missing parts array"));
            };
            let parts: Vec<String> = parts
                .iter()
                .map(|part| part.as_str().unwrap_or("").trim().to_owned())
                .collect();
            if parts.len() < 2 {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain(
                        "kind_say needs at least 2 parts; answer in one text bubble instead",
                    ),
                );
            }
            if parts.iter().any(String::is_empty) {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain("kind_say parts must all be non-empty"),
                );
            }
            ToolResult::success_with_meta(
                call.id,
                format!("Said {} parts.", parts.len()),
                json!({"kind_say": {"parts": parts}}),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use tools::ToolSessionState;

    fn context() -> ToolContext {
        ToolContext::with_session(
            "/tmp",
            Arc::new(ToolSessionState::new(
                std::path::Path::new("/tmp"),
                "kind-say-test",
            )),
        )
    }

    fn user_event(seq: u64, turn: u64, rpc_id: &str) -> session::Event {
        session::Event::new(
            seq,
            1,
            session::EventKind::UserMessage {
                turn,
                step: 0,
                message: model::Message::user_text_with_request_id("谢谢", rpc_id),
            },
        )
    }

    fn context_with_events(turn: u64, events: Vec<session::Event>) -> ToolContext {
        let events = Arc::new(events);
        context()
            .with_turn(turn)
            .observing_session(Arc::new(move || (*events).clone()))
    }

    fn call(id: &str, name: &str, arguments: &str) -> ToolCall {
        ToolCall {
            id: id.to_owned(),
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        }
    }

    #[tokio::test]
    async fn react_accepts_only_the_eleven_ids() {
        let tool = KindReactTool::new();
        let ctx = context_with_events(1, vec![user_event(1, 1, "u1")]);
        let ok = tool
            .execute(call("1", "kind_react", r#"{"emoji_id":"+1"}"#), &ctx)
            .await;
        assert!(!ok.is_error);
        assert_eq!(ok.meta.unwrap()["kind_react"]["emoji_id"], "+1");
        let bad = tool
            .execute(call("2", "kind_react", r#"{"emoji_id":"smile"}"#), &ctx)
            .await;
        assert!(bad.is_error);
    }

    #[tokio::test]
    async fn react_skips_program_messages_and_needs_a_user_message_this_turn() {
        let tool = KindReactTool::new();
        let ctx = context_with_events(
            2,
            vec![
                user_event(1, 1, "u1"),
                user_event(2, 2, "u2"),
                user_event(3, 2, "fixed-block:abc"),
                user_event(4, 2, "task-report:t:r"),
            ],
        );
        let ok = tool
            .execute(call("1", "kind_react", r#"{"emoji_id":"+1"}"#), &ctx)
            .await;
        assert!(!ok.is_error);
        assert_eq!(ok.meta.unwrap()["kind_react"]["user_message_seq"], 2);

        let no_user = context_with_events(3, vec![user_event(4, 3, "fixed-block:abc")]);
        let error = tool
            .execute(call("2", "kind_react", r#"{"emoji_id":"+1"}"#), &no_user)
            .await;
        assert!(error.is_error);
        assert!(error.content.contains("这一轮没有用户的消息可以回"));
    }

    #[tokio::test]
    async fn say_requires_two_non_empty_parts_and_streams_meta() {
        let tool = KindSayTool::new();
        let bad = tool
            .execute(
                call("1", "kind_say", r#"{"parts":["只有一条"]}"#),
                &context(),
            )
            .await;
        assert!(bad.is_error);
        let empty = tool
            .execute(
                call("2", "kind_say", r#"{"parts":["一条","  "]} "#),
                &context(),
            )
            .await;
        assert!(empty.is_error);
        let ok = tool
            .execute(
                call("3", "kind_say", r#"{"parts":["第一段","第二段"]}"#),
                &context(),
            )
            .await;
        assert!(!ok.is_error);
        assert_eq!(ok.meta.unwrap()["kind_say"]["parts"][1], "第二段");
    }
}
