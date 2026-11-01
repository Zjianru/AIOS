//! 把 provider 无关的消息序列化成 Messages 协议的请求体。

use serde::Serialize;
use serde_json::Value;

use crate::{ContentBlock, ErrorCode, Message, ModelError, Role, ToolSchema};

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
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<WireTool>>,
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
struct WireTool {
    name: String,
    description: String,
    input_schema: Value,
}

#[derive(Serialize)]
struct WireMessage {
    role: &'static str,
    content: Vec<WireBlock>,
}

#[derive(Serialize)]
struct WireText {
    #[serde(rename = "type")]
    kind: &'static str,
    text: String,
}

#[derive(Serialize)]
#[serde(tag = "type")]
enum WireBlock {
    #[serde(rename = "text")]
    Text { text: String },
    #[serde(rename = "thinking")]
    Thinking {
        thinking: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    #[serde(rename = "tool_use")]
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
    #[serde(rename = "tool_result")]
    ToolResult {
        tool_use_id: String,
        content: Vec<WireText>,
        #[serde(skip_serializing_if = "Option::is_none")]
        is_error: Option<bool>,
    },
}

/// 序列化一次请求。
///
/// 系统消息合并成 system 字段；tool_call / tool_result 分别走 wire 的 tool_use /
/// tool_result 块；用户、助手、工具消息按 wire 角色合并相邻项（照 DSH 的 serialize）；
/// 空文本块和空消息跳过。
pub(super) fn request_body(
    config: &DeepSeekConfig,
    messages: &[Message],
    tools: &[ToolSchema],
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
                        ContentBlock::Reasoning { .. } | ContentBlock::ToolCall { .. } => {
                            return Err(unsupported("system 消息只能包含文本块"));
                        }
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
            // DSH 的 developer 消息目前是 log-only（工具增删记录），AIOS 还没有生产者；
            // 真出现也不进请求，按不支持内容报错。
            Role::Developer => return Err(unsupported("developer 消息不进请求")),
            Role::User | Role::Assistant => {
                let role = match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                    Role::System | Role::Tool | Role::Developer => {
                        unreachable!("上面已单独处理")
                    }
                };
                let content: Vec<WireBlock> = message
                    .content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            Some(WireBlock::Text { text: text.clone() })
                        }
                        ContentBlock::Text { .. } => None,
                        ContentBlock::Reasoning { text, signature }
                            if message.role == Role::Assistant =>
                        {
                            Some(WireBlock::Thinking {
                                thinking: text.clone(),
                                signature: signature.clone(),
                            })
                        }
                        ContentBlock::Reasoning { .. } => None,
                        ContentBlock::ToolCall {
                            id,
                            name,
                            arguments,
                        } if message.role == Role::Assistant => {
                            let input = serde_json::from_str::<Value>(arguments)
                                .ok()
                                .filter(Value::is_object)
                                .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                            Some(WireBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input,
                            })
                        }
                        ContentBlock::ToolCall { .. } => None,
                    })
                    .collect();
                if content.is_empty() {
                    continue;
                }
                push_or_merge(&mut wire_messages, role, content);
            }
            // DSH ToolResultMessage：toolCallId/isError 在消息级，content 是结果内容块；
            // wire 上是一条 user 消息里的 tool_result 块。
            Role::Tool => {
                let Some(tool_use_id) = message.tool_call_id.as_ref() else {
                    return Err(unsupported("tool 消息缺少 toolCallId"));
                };
                let mut content = Vec::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text } if !text.is_empty() => {
                            content.push(WireText {
                                kind: "text",
                                text: text.clone(),
                            });
                        }
                        ContentBlock::Text { .. } => {}
                        ContentBlock::Reasoning { .. } | ContentBlock::ToolCall { .. } => {
                            return Err(unsupported("tool 消息只能包含文本内容块"));
                        }
                    }
                }
                push_or_merge(
                    &mut wire_messages,
                    "user",
                    vec![WireBlock::ToolResult {
                        tool_use_id: tool_use_id.clone(),
                        content,
                        is_error: if message.is_error.unwrap_or(false) {
                            Some(true)
                        } else {
                            None
                        },
                    }],
                );
            }
        }
    }

    let effort = config.effort;
    let wire_tools = if tools.is_empty() {
        None
    } else {
        Some(
            tools
                .iter()
                .map(|tool| WireTool {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.parameters.clone(),
                })
                .collect(),
        )
    };
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
        tools: wire_tools,
    };
    serde_json::to_vec(&body).map_err(|error| {
        ModelError::new(
            ErrorCode::InvalidRequest,
            format!("请求序列化失败: {error}"),
        )
    })
}

fn push_or_merge(messages: &mut Vec<WireMessage>, role: &'static str, content: Vec<WireBlock>) {
    if let Some(previous) = messages.last_mut()
        && previous.role == role
    {
        previous.content.extend(content);
        return;
    }
    messages.push(WireMessage { role, content });
}

fn unsupported(message: &str) -> ModelError {
    ModelError::new(ErrorCode::UnsupportedContent, message.to_owned())
}
