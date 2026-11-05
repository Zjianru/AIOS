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
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<WireThinking>,
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
    #[serde(skip_serializing_if = "Option::is_none")]
    budget_tokens: Option<u32>,
}

#[derive(Serialize)]
struct WireOutputConfig {
    effort: String,
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

/// T013.4：文件块的模型可见文本（照 DSH \`llm/llm/src/content.ts\` 的 fileHandleText 原句）。
///
/// `path` 是只读副本的绝对路径；没有路径时用 DSH 的「环境不可读」分支。
pub fn file_handle_text(
    _attachment_id: &str,
    _name: &str,
    _bytes: u64,
    _path: Option<&str>,
) -> String {
    // T013.4 红阶段：实现前空壳（fileHandleText 原句还没填）。
    String::new()
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
    max_tokens: Option<u32>,
    validate_pairing: bool,
) -> Result<Vec<u8>, ModelError> {
    if validate_pairing {
        validate_tool_pairing(messages)?;
    }
    let mut system = String::new();
    let mut wire_messages: Vec<WireMessage> = Vec::new();
    for message in messages {
        match message.role {
            Role::System => {
                let mut text = String::new();
                for block in &message.content {
                    match block {
                        ContentBlock::Text { text: piece, .. } => text.push_str(piece),
                        ContentBlock::Reasoning { .. }
                        | ContentBlock::ToolCall { .. }
                        | ContentBlock::File { .. } => {
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
                        ContentBlock::Text { text, .. } if !text.is_empty() => {
                            Some(WireBlock::Text { text: text.clone() })
                        }
                        ContentBlock::Text { .. } => None,
                        // T013.4：文件块投给模型时换成 fileHandleText（DSH 原句照抄）。
                        ContentBlock::File {
                            attachment_id,
                            name,
                            bytes,
                            path,
                        } if message.role == Role::User => Some(WireBlock::Text {
                            text: file_handle_text(attachment_id, name, *bytes, path.as_deref()),
                        }),
                        ContentBlock::File { .. } => None,
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
                        ContentBlock::Text { text, .. } if !text.is_empty() => {
                            content.push(WireText {
                                kind: "text",
                                text: text.clone(),
                            });
                        }
                        ContentBlock::Text { .. } => {}
                        ContentBlock::Reasoning { .. }
                        | ContentBlock::ToolCall { .. }
                        | ContentBlock::File { .. } => {
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

    let effort = &config.effort;
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
    let wire = super::vendors::wire_effort(
        &config.base_url,
        &config.model,
        effort,
        max_tokens,
        config.max_tokens,
    );
    let thinking = wire.thinking.map(|thinking| match thinking {
        super::vendors::ThinkingWire::Disabled => WireThinking {
            kind: "disabled",
            budget_tokens: None,
        },
        super::vendors::ThinkingWire::Enabled { budget_tokens } => WireThinking {
            kind: "enabled",
            budget_tokens: Some(budget_tokens),
        },
    });
    let body = WireRequest {
        model: config.model.clone(),
        stream: true,
        messages: wire_messages,
        max_tokens: wire.max_tokens,
        thinking,
        output_config: wire.output_effort.map(|effort| WireOutputConfig { effort }),
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

/// DSH DeepSeek 序列化的三类防御校验（T006.19 照 llm-deepseek/src/serialize.ts:136-146）：
/// 重复 tool call id、tool_result 没有匹配的调用、历史结束时仍有未决工具调用。
fn validate_tool_pairing(messages: &[Message]) -> Result<(), ModelError> {
    // 压缩 checkpoint 存在时，surface 的替换/重排语义会让配对检查不再可靠
    // （见 unmatched result 分支的说明）；只对没有 checkpoint 的普通历史做完整防御。
    let has_checkpoint = messages.iter().any(|message| {
        matches!(
            message.source,
            crate::MessageSource::CompactCheckpoint { .. }
        )
    });
    let mut pending: Vec<&str> = Vec::new();
    for message in messages {
        match message.role {
            Role::Assistant => {
                for block in &message.content {
                    let ContentBlock::ToolCall { id, .. } = block else {
                        continue;
                    };
                    if pending.contains(&id.as_str()) {
                        return Err(invalid_request("DeepSeek Messages duplicate tool call id"));
                    }
                    pending.push(id.as_str());
                }
            }
            Role::Tool => {
                let Some(tool_use_id) = message.tool_call_id.as_deref() else {
                    continue;
                };
                let Some(index) = pending.iter().position(|id| *id == tool_use_id) else {
                    // AIOS 的压缩 checkpoint 会把旧区间从 surface 隐藏，而 pruner 替换节点
                    // 按 append 顺序落在 surface 末尾；因此压缩过的历史里可能出现「结果还在、
                    // 对应调用已被 checkpoint 隐藏」的合法形状（T006.19 适配）。
                    if has_checkpoint {
                        continue;
                    }
                    return Err(invalid_request(
                        "DeepSeek Messages tool result has no matching call",
                    ));
                };
                pending.remove(index);
            }
            Role::System | Role::User | Role::Developer => {}
        }
    }
    if !pending.is_empty() {
        if has_checkpoint {
            return Ok(());
        }
        return Err(invalid_request(
            "DeepSeek Messages history ends with unresolved tools",
        ));
    }
    Ok(())
}

fn invalid_request(message: &str) -> ModelError {
    ModelError::new(ErrorCode::InvalidRequest, message.to_owned())
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

/// T006.19 红测试：DSH DeepSeek 序列化的三类防御校验
/// （llm-deepseek/src/serialize.ts:136-146：重复 tool call id / tool_result 匹配 / 历史未决工具）。
#[cfg(test)]
mod t006_19_serialize_defense_tests {
    use super::*;
    use crate::{ContentBlock, ErrorCode, Message};

    fn assistant(tool_call_ids: &[(&str, &str)]) -> Message {
        let content: Vec<ContentBlock> = tool_call_ids
            .iter()
            .map(|(id, name)| ContentBlock::ToolCall {
                id: (*id).to_owned(),
                name: (*name).to_owned(),
                arguments: "{}".to_owned(),
            })
            .collect();
        Message::assistant(content, "deepseek", "deepseek-flash")
    }

    fn tool_result(id: &str) -> Message {
        Message::tool_result(id, "ok", false)
    }

    #[test]
    fn duplicate_tool_call_ids_are_rejected() {
        let messages = vec![assistant(&[("call_dup", "bash"), ("call_dup", "read")])];
        let error = validate_tool_pairing(&messages).expect_err("重复 callId 必须被拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("duplicate tool call id"),
            "文案照 DSH：{}",
            error.message
        );
    }

    #[test]
    fn tool_result_without_matching_call_is_rejected() {
        let messages = vec![assistant(&[("call_ok", "bash")]), tool_result("call_other")];
        let error = validate_tool_pairing(&messages).expect_err("孤儿 tool_result 必须被拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("tool result has no matching call"),
            "文案照 DSH：{}",
            error.message
        );
    }

    #[test]
    fn history_with_unresolved_tools_is_rejected() {
        let messages = vec![assistant(&[("call_open", "bash")])];
        let error = validate_tool_pairing(&messages).expect_err("历史末尾未决工具必须被拒");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("history ends with unresolved tools"),
            "文案照 DSH：{}",
            error.message
        );
    }

    #[test]
    fn well_formed_tool_pairs_pass() {
        let messages = vec![assistant(&[("call_ok", "bash")]), tool_result("call_ok")];
        assert!(
            validate_tool_pairing(&messages).is_ok(),
            "成对的 tool_use/tool_result 不能被误拒"
        );
    }
}

#[cfg(test)]
mod effort_wire_tests {
    use super::*;
    use crate::{DeepSeekConfig, Effort};
    use serde_json::Value;

    fn body(base_url: &str, model: &str, effort: &str, max_tokens: Option<u32>) -> Value {
        let config = DeepSeekConfig {
            base_url: base_url.to_owned(),
            model: model.to_owned(),
            max_tokens: 256_000,
            context_window: 1_000_000,
            effort: Effort::new(effort),
        };
        let raw = request_body(&config, &[], &[], max_tokens, false).expect("序列化");
        serde_json::from_slice(&raw).expect("JSON")
    }

    fn thinking(value: &Value) -> Option<(&str, Option<u64>)> {
        value.get("thinking").map(|thinking| {
            (
                thinking["type"].as_str().expect("thinking.type"),
                thinking.get("budget_tokens").and_then(Value::as_u64),
            )
        })
    }

    fn output_effort(value: &Value) -> Option<&str> {
        value
            .get("output_config")
            .and_then(|config| config.get("effort"))
            .and_then(Value::as_str)
    }

    #[test]
    fn deepseek_levels_serialize_thinking_budget_and_output_effort() {
        let table: &[(&str, &str, Option<u64>, Option<&str>)] = &[
            ("none", "disabled", None, None),
            ("low", "enabled", Some(4096), Some("low")),
            ("high", "enabled", Some(24_000), Some("high")),
            ("xhigh", "enabled", Some(32_000), Some("max")),
            ("max", "enabled", Some(32_000), Some("max")),
        ];
        for (effort, kind, budget, output) in table {
            let value = body(
                "https://api.deepseek.com/anthropic",
                "deepseek-flash",
                effort,
                Some(64_000),
            );
            assert_eq!(thinking(&value), Some((*kind, *budget)), "effort={effort}");
            assert_eq!(output_effort(&value), *output, "effort={effort}");
            assert_eq!(value["max_tokens"], 64_000);
        }
    }

    #[test]
    fn minimax_m31_always_thinks_but_uses_lowest_level_for_none() {
        let value = body(
            "https://api.minimax.cn/anthropic",
            "MiniMax-M3.1-Flash",
            "none",
            Some(64_000),
        );
        assert_eq!(thinking(&value), Some(("enabled", Some(4096))));
        assert_eq!(output_effort(&value), Some("low"));

        let value = body(
            "https://api.minimax.cn/anthropic",
            "MiniMax-M3.1-Flash",
            "xhigh",
            Some(64_000),
        );
        assert_eq!(thinking(&value), Some(("enabled", Some(32_000))));
        assert_eq!(output_effort(&value), Some("xhigh"));
    }

    #[test]
    fn budget_only_models_do_not_get_output_effort() {
        let value = body(
            "https://api.minimax.cn/anthropic",
            "MiniMax-M3",
            "high",
            Some(64_000),
        );
        assert_eq!(thinking(&value), Some(("enabled", Some(24_000))));
        assert_eq!(output_effort(&value), None);

        let value = body(
            "https://relay.example.com/anthropic",
            "deepseek-flash",
            "high",
            Some(64_000),
        );
        assert_eq!(thinking(&value), Some(("enabled", Some(24_000))));
        assert_eq!(
            output_effort(&value),
            None,
            "别的地址上的 DeepSeek 不当自己人"
        );
    }

    #[test]
    fn tiny_max_tokens_turns_thinking_off() {
        let value = body(
            "https://api.deepseek.com/anthropic",
            "deepseek-flash",
            "high",
            Some(2048),
        );
        assert_eq!(thinking(&value), None);
        assert_eq!(output_effort(&value), None);
        assert_eq!(value["max_tokens"], 2048);
    }

    #[test]
    fn no_caller_max_tokens_raises_it_above_the_budget() {
        let value = body(
            "https://api.deepseek.com/anthropic",
            "deepseek-flash",
            "max",
            None,
        );
        assert_eq!(value["max_tokens"], 256_000);
    }
}
