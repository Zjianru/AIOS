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
            let user_message_seq =
                context
                    .session_events()
                    .iter()
                    .rev()
                    .find_map(|event| match &event.kind {
                        session::EventKind::UserMessage { message, .. }
                            if matches!(message.source, model::MessageSource::User { .. }) =>
                        {
                            Some(event.seq)
                        }
                        _ => None,
                    });
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
                .filter(|part| !part.is_empty())
                .collect();
            if parts.len() < 2 {
                return ToolResult::error(
                    call.id,
                    ToolFailure::plain(
                        "kind_say needs at least 2 non-empty parts; answer in one text bubble instead",
                    ),
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
        let ok = tool
            .execute(call("1", "kind_react", r#"{"emoji_id":"+1"}"#), &context())
            .await;
        assert!(!ok.is_error);
        assert_eq!(ok.meta.unwrap()["kind_react"]["emoji_id"], "+1");
        let bad = tool
            .execute(
                call("2", "kind_react", r#"{"emoji_id":"smile"}"#),
                &context(),
            )
            .await;
        assert!(bad.is_error);
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
