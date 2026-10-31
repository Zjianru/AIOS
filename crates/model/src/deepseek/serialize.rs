//! 把 provider 无关的消息序列化成 Messages 协议的请求体。

use serde::Serialize;

use crate::{ContentBlock, ErrorCode, Message, ModelError, Role};

use super::config::DeepSeekConfig;

#[derive(Serialize)]
struct WireRequest {
    model: String,
    stream: bool,
    messages: Vec<WireMessage>,
    max_tokens: u32,
    thinking: WireThinking,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<WireOutputConfig>,
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<String>,
}

#[derive(Serialize)]
struct WireThinking {
    #[serde(rename = "type")]
    kind: &'static str,
}

#[derive(Serialize)]
struct WireOutputConfig {
    effort: &'static str,
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireBlock>,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum WireBlock {
    #[serde(rename = "text")]
    Text { text: String },
}

/// 序列化一次请求。
///
/// 系统消息合并成 `system` 字段；用户和助手消息按角色合并相邻项（照 DSH 的
/// `serialize`）；空文本块和空消息跳过。
pub(super) fn request_body(
    config: &DeepSeekConfig,
    messages: &[Message],
) -> Result<Vec<u8>, ModelError> {
    let mut system = String::new();
    let mut wire_messages: Vec<WireMessage> = Vec::new();
    for message in messages {
        match message.role {
            Role::System => {
                let mut text = String::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text: piece } => text.push_str(piece),
                    }
                }
                if text.is_empty() {
                    continue;
                }
                if !system.is_empty() {
                    system.push_str("\n\n");
                }
                system.push_str(&text);
            }
            Role::User | Role::Assistant => {
                let role = match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System => unreachable!("上面已单独处理"),
                };
                let content: Vec<WireBlock> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            Some(WireBlock::Text { text: text.clone() })
                        }
                        ContentBlock::Text { .. } => None,
                    })
                    .collect();
                if content.is_empty() {
                    continue;
                }
                if let Some(previous) = wire_messages.last_mut()
                    && previous.role == role
                {
                    previous.content.extend(content);
                    continue;
                }
                wire_messages.push(WireMessage { role, content });
            }
        }
    }

    let effort = config.effort;
    let body = WireRequest {
        model: config.model.clone(),
        stream: true,
        messages: wire_messages,
        max_tokens: config.max_tokens,
        thinking: WireThinking {
            kind: if effort == crate::Effort::Off {
                "disabled"
            } else {
                "enabled"
            },
        },
        output_config: if effort == crate::Effort::Off {
            None
        } else {
            Some(WireOutputConfig {
                effort: effort.as_wire(),
            })
        },
        system: if system.is_empty() {
            None
        } else {
            Some(system)
        },
    };
    serde_json::to_vec(&body).map_err(|error| {
        ModelError::new(
            ErrorCode::InvalidRequest,
            format!("请求序列化失败: {error}"),
        )
    })
}
