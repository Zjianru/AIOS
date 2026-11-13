//! 把 provider 无关的消息序列化成 Messages 协议的请求体。

use serde::Serialize;
use serde_json::Value;

use crate::{ContentBlock, ErrorCode, Message, ModelError, Role, ToolRequest, ToolUpdate};

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
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    defer_loading: bool,
}

/// `tool_addition` 块里对工具定义的引用（DSH 的 `tool_reference`）。
#[derive(Serialize)]
struct WireToolReference {
    #[serde(rename = "type")]
    kind: &'static str,
    name: String,
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

/// 文件块的模型可见文本（照 DSH \`llm/llm/src/content.ts\` 的 fileHandleText 原句）。
///
/// `path` 是只读副本的绝对路径；没有路径时用 DSH 的「环境不可读」分支。
pub fn file_handle_text(attachment_id: &str, name: &str, bytes: u64, path: Option<&str>) -> String {
    let digest: String = attachment_id
        .strip_prefix("sha256:")
        .unwrap_or(attachment_id)
        .chars()
        .take(8)
        .collect();
    let quoted = |value: &str| serde_json::to_string(value).unwrap_or_default();
    let identity = format!("File {} ({} bytes, sha256:{})", quoted(name), bytes, digest);
    match path {
        Some(path) => format!(
            "[{}: verbatim read-only copy saved at {}. Read that path with your file tools when its contents are needed; copy it to a writable location before modifying it. When delegating file work, include this saved path in the delegation prompt; only subagents sharing this execution environment can read it.]",
            identity,
            quoted(path)
        ),
        None => format!(
            "[{identity} was uploaded, but the current execution environment cannot access a readable path. Report that limitation if its contents are needed; do not claim to have read it.]"
        ),
    }
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
    /// 历史里的一次工具加载：把带 `defer_loading` 的声明激活（DSH 的 tool_addition 块）。
    #[serde(rename = "tool_addition")]
    ToolAddition { tool: WireToolReference },
}

/// 序列化一次请求。
///
/// 开头的静态 system 消息（第一个非 system 消息之前）合并成 system 字段；之后历史中段的
/// 动态 system（卡片状态回执等）按原位置落成 user 角色的文本消息——system 全程不变、
/// 对话前缀字节稳定，缓存不会因为一条回执整段重算（T045）。
/// tool_call / tool_result 分别走 wire 的 tool_use / tool_result 块；用户、助手、工具消息
/// 按 wire 角色合并相邻项（照 DSH 的 serialize）；空文本块和空消息跳过。
pub(super) fn request_body(
    config: &DeepSeekConfig,
    messages: &[Message],
    tools: &ToolRequest,
    tool_update: Option<ToolUpdate>,
    max_tokens: Option<u32>,
    validate_pairing: bool,
) -> Result<PreparedRequest, ModelError> {
    if validate_pairing {
        validate_tool_pairing(messages)?;
    }
    let projected = crate::tool_updates::project(messages, tools, tool_update);
    let mut system = String::new();
    let mut wire_messages: Vec<WireMessage> = Vec::new();
    // T045：第一个非 system 消息之后的 system 是动态回执，不再并进 top-level system。
    let mut seen_non_system = false;
    for (index, message) in messages.iter().enumerate() {
        if !matches!(message.role, Role::System) {
            seen_non_system = true;
        }
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
                if seen_non_system {
                    // 历史中段的动态 system（卡片状态回执）：按原位置换成 user 文本，
                    // 内容逐字保留；top-level system 不再增长。
                    push_or_merge(&mut wire_messages, "user", vec![WireBlock::Text { text }]);
                } else {
                    if !system.is_empty() {
                        system.push_str("\n\n");
                    }
                    system.push_str(&text);
                }
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
                    .map(|block| -> Result<Option<WireBlock>, ModelError> {
                        Ok(match block {
                            ContentBlock::Text { text, .. } if !text.is_empty() => {
                                Some(WireBlock::Text { text: text.clone() })
                            }
                            ContentBlock::Text { .. } => None,
                            // 文件块投给模型时换成 fileHandleText（DSH 原句照抄）。
                            ContentBlock::File {
                                attachment_id,
                                name,
                                bytes,
                                path,
                            } if message.role == Role::User => Some(WireBlock::Text {
                                text: file_handle_text(
                                    attachment_id,
                                    name,
                                    *bytes,
                                    path.as_deref(),
                                ),
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
                            } if message.role == Role::Assistant => Some(WireBlock::ToolUse {
                                id: id.clone(),
                                name: name.clone(),
                                input: tool_input(id, arguments)?,
                            }),
                            ContentBlock::ToolCall { .. } => None,
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
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
        // 加载点插一条 system 角色的 tool_addition（只在声明了能力的路由上；空表 = 不插）。
        // 位置必须紧跟 user/tool-result 回合，和 DSH 的 flushSystemUpdates 形状一致。
        if let Some(names) = projected.insertions.get(&index) {
            if !matches!(message.role, Role::User | Role::Tool) {
                return Err(invalid_request(
                    "tool addition must follow a user or tool-result turn",
                ));
            }
            let content = names
                .iter()
                .map(|name| WireBlock::ToolAddition {
                    tool: WireToolReference {
                        kind: "tool_reference",
                        name: name.clone(),
                    },
                })
                .collect();
            wire_messages.push(WireMessage {
                role: "system",
                content,
            });
        }
    }

    let effort = &config.effort;
    let wire_tools = if projected.tools.is_empty() {
        None
    } else {
        Some(
            projected
                .tools
                .iter()
                .map(|tool| WireTool {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    input_schema: tool.parameters.clone(),
                    defer_loading: tool.defer_loading,
                })
                .collect(),
        )
    };
    let mut wire = super::vendors::wire_effort(
        &config.base_url,
        &config.model,
        effort,
        max_tokens,
        config.max_tokens,
    );
    // 请求的输出上限不能超过这个模型自己声明的上限。装配时的全局上限（256k）
    // 是初值；换到上限更小的模型（MiniMax-M2.7 这类）后照旧发会被厂商按 400 拒绝。
    // 目录里没有这个模型（自定义连接）就不改。
    if let Some(limit) = crate::catalog::output_limit(&config.provider, &config.model) {
        wire.max_tokens = wire.max_tokens.min(limit);
    }
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
    let body = serde_json::to_vec(&body).map_err(|error| {
        ModelError::new(
            ErrorCode::InvalidRequest,
            format!("请求序列化失败: {error}"),
        )
    })?;
    Ok(PreparedRequest {
        body,
        tool_changes: !projected.insertions.is_empty(),
    })
}

/// 序列化好的一次请求：请求体 + 是否需要工具变更 beta 头（照 DSH：有 tool_addition 块才加）。
pub(super) struct PreparedRequest {
    pub body: Vec<u8>,
    pub tool_changes: bool,
}

/// DSH DeepSeek 序列化的三类防御校验（照 llm-deepseek/src/serialize.ts:136-146）
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
                    // 对应调用已被 checkpoint 隐藏」的合法形状（适配）。
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

/// 历史里的 tool_use 参数：必须是 JSON 对象。坏参数不能静默变成空参数——模型会以为工具
/// 当时没收到参数，重放出来的请求也就不是这条历史了。
fn tool_input(id: &str, arguments: &str) -> Result<Value, ModelError> {
    match serde_json::from_str::<Value>(arguments) {
        Ok(value) if value.is_object() => Ok(value),
        _ => Err(invalid_request(&format!(
            "工具调用 {id} 的 arguments 不是 JSON 对象，请求体不能忠实重建"
        ))),
    }
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

/// 红测试：DSH DeepSeek 序列化的三类防御校验
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

    /// 历史里的 tool_use 参数不是 JSON 时，请求体不能静默变成空参数：报错并指名是哪次调用。
    #[test]
    fn bad_json_signal() {
        let config = crate::DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "https://api.deepseek.com/anthropic".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 256_000,
            context_window: 1_000_000,
            effort: crate::Effort::new("low"),
            tool_update: None,
        };
        let messages = vec![Message::assistant(
            vec![ContentBlock::ToolCall {
                id: "call_bad".to_owned(),
                name: "bash".to_owned(),
                arguments: "{not json".to_owned(),
            }],
            "deepseek",
            "deepseek-flash",
        )];
        let error = match request_body(
            &config,
            &messages,
            &crate::ToolRequest::default(),
            None,
            None,
            false,
        ) {
            Ok(prepared) => panic!(
                "坏参数必须报错，不能静默变成空参数：{}",
                String::from_utf8_lossy(&prepared.body)
            ),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("call_bad"),
            "报错要指名是哪次工具调用：{}",
            error.message
        );
    }
}

#[cfg(test)]
mod effort_wire_tests {
    use super::*;
    use crate::{DeepSeekConfig, Effort};
    use serde_json::Value;

    fn body(base_url: &str, model: &str, effort: &str, max_tokens: Option<u32>) -> Value {
        with_provider("deepseek", base_url, model, effort, max_tokens)
    }

    fn with_provider(
        provider: &str,
        base_url: &str,
        model: &str,
        effort: &str,
        max_tokens: Option<u32>,
    ) -> Value {
        let config = DeepSeekConfig {
            provider: provider.to_owned(),
            base_url: base_url.to_owned(),
            model: model.to_owned(),
            max_tokens: 256_000,
            context_window: 1_000_000,
            effort: Effort::new(effort),
            tool_update: None,
        };
        let prepared = request_body(
            &config,
            &[],
            &crate::ToolRequest::default(),
            None,
            max_tokens,
            false,
        )
        .expect("序列化");
        serde_json::from_slice(&prepared.body).expect("JSON")
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

    /// 换到输出上限更小的模型以后，请求的 max_tokens 不能超过目录给它的上限
    /// （MiniMax-M2.7 会 400：max tokens > 196608）；上限更大的模型保持装配时的值。
    #[test]
    fn request_max_tokens_never_exceed_the_model_output_limit() {
        let m27 = with_provider(
            "minimax-cn",
            "https://api.minimaxi.com/anthropic",
            "MiniMax-M2.7",
            "low",
            None,
        );
        assert_eq!(m27["max_tokens"], 131_072);
        let flash = with_provider(
            "minimax-cn",
            "https://api.minimaxi.com/anthropic",
            "MiniMax-M3.1-Flash",
            "low",
            None,
        );
        assert_eq!(flash["max_tokens"], 256_000);
    }

    /// 目录里没有这个模型（自定义连接）时不动装配时的输出上限。
    #[test]
    fn request_max_tokens_stay_when_catalog_has_no_model() {
        let custom = with_provider(
            "custom",
            "http://127.0.0.1:1",
            "private-model",
            "low",
            Some(64_000),
        );
        assert_eq!(custom["max_tokens"], 64_000, "目录没有该模型时不改请求上限");
        let default_limit =
            with_provider("custom", "http://127.0.0.1:1", "private-model", "low", None);
        assert_eq!(default_limit["max_tokens"], 256_000);
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

#[cfg(test)]
mod t013_file_block_tests {
    use super::*;

    /// fileHandleText 原句（DSH llm/llm/src/content.ts:151-158 照抄）。
    #[test]
    fn file_handle_text_matches_dsh_verbatim() {
        let path = "/tmp/data/attachments/v1/files/ab/abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567/notes.txt";
        let text = file_handle_text(
            "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567",
            "notes.txt",
            12,
            Some(path),
        );
        assert_eq!(
            text,
            format!(
                "[File \"notes.txt\" (12 bytes, sha256:abcdef01): verbatim read-only copy saved at \"{path}\". Read that path with your file tools when its contents are needed; copy it to a writable location before modifying it. When delegating file work, include this saved path in the delegation prompt; only subagents sharing this execution environment can read it.]"
            ),
            "fileHandleText 要和 DSH 一字不差"
        );
    }

    #[test]
    fn file_handle_text_without_a_path_uses_the_uploaded_branch() {
        let text = file_handle_text("sha256:abcdef0123456789", "a b.txt", 3, None);
        assert_eq!(
            text,
            "[File \"a b.txt\" (3 bytes, sha256:abcdef01) was uploaded, but the current execution environment cannot access a readable path. Report that limitation if its contents are needed; do not claim to have read it.]"
        );
    }

    /// 粘贴块照原文进请求体；文件块换成 fileHandleText。
    #[test]
    fn request_body_keeps_pastes_verbatim_and_replaces_files_with_handle_text() {
        let config = DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "http://127.0.0.1:1".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 2048,
            context_window: 128_000,
            effort: crate::Effort::new("high"),
            tool_update: None,
        };
        let message = Message::user_with_request_id(
            vec![
                ContentBlock::text("看这两个"),
                ContentBlock::paste("第一段\n粘贴原文  "),
                ContentBlock::paste("第二段"),
                ContentBlock::File {
                    attachment_id:
                        "sha256:abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567"
                            .to_owned(),
                    name: "notes.txt".to_owned(),
                    bytes: 12,
                    path: Some("/tmp/data/attachments/v1/files/ab/x/notes.txt".to_owned()),
                },
            ],
            "req-1",
        );
        let prepared = super::request_body(
            &config,
            &[message],
            &crate::ToolRequest::default(),
            None,
            Some(2048),
            true,
        )
        .expect("序列化");
        let json: Value = serde_json::from_slice(&prepared.body).unwrap();
        let content = json["messages"][0]["content"].as_array().unwrap();
        assert_eq!(content[0]["text"], "看这两个");
        assert_eq!(
            content[1]["text"], "第一段\n粘贴原文  ",
            "粘贴原文不截断不改写"
        );
        assert_eq!(content[2]["text"], "第二段");
        let expected = "[File \"notes.txt\" (12 bytes, sha256:abcdef01): verbatim read-only copy saved at \"/tmp/data/attachments/v1/files/ab/x/notes.txt\". Read that path with your file tools when its contents are needed; copy it to a writable location before modifying it. When delegating file work, include this saved path in the delegation prompt; only subagents sharing this execution environment can read it.]";
        assert_eq!(content[3]["text"], expected, "文件块换成 fileHandleText");
    }
}

/// T045：卡片状态回执等「历史中段的动态 system」不能并进 top-level system（会让缓存前缀
/// 每轮平移）；它们按原历史位置序列化成 user 角色的文本消息，开头静态 system 仍进 system。
#[cfg(test)]
mod t045_cache_safe_system_tests {
    use super::*;
    use crate::{
        ContentBlock, Message, MessageId, ToolAddition, ToolHistory, ToolRequest, ToolSchema,
        ToolUpdate,
    };

    const PREAMBLE: &str = "Latest status of action cards from user actions and background work. These are status facts, not a request to start or retry work. Use these receipts instead of the earlier proposal when describing progress.";
    const CLOSING: &str = "Continue with the user's current request.";

    fn config() -> crate::DeepSeekConfig {
        crate::DeepSeekConfig {
            provider: "deepseek".to_owned(),
            base_url: "https://api.deepseek.com/anthropic".to_owned(),
            model: "deepseek-flash".to_owned(),
            max_tokens: 256_000,
            context_window: 1_000_000,
            effort: crate::Effort::new("low"),
            tool_update: Some(ToolUpdate::AdditionOnly),
        }
    }

    fn body(messages: &[Message], tools: &ToolRequest) -> Value {
        let prepared = request_body(
            &config(),
            messages,
            tools,
            Some(ToolUpdate::AdditionOnly),
            None,
            false,
        )
        .expect("序列化");
        serde_json::from_slice(&prepared.body).expect("请求体是 JSON")
    }

    /// 历史中段的 system（卡片回执）必须是 user 角色的文本消息，原文一字不改；
    /// 开头第一条仍只在 top-level system。
    #[test]
    fn tail_system_receipt_becomes_user_text() {
        let receipt = format!(
            "{PREAMBLE} {CLOSING}\n{{\"kind\":\"action_cards\",\"cards\":[{{\"id\":\"c1\",\"status\":\"accepted\"}}]}}"
        );
        let messages = vec![
            Message::system_prompt("静态系统提示"),
            Message::text(Role::User, "把卡片收据发给我"),
            Message::assistant(
                vec![ContentBlock::text("好的")],
                "deepseek",
                "deepseek-flash",
            ),
            Message::text(Role::System, receipt.clone()),
            Message::text(Role::User, "接着做"),
        ];
        let json = body(&messages, &ToolRequest::default());
        assert_eq!(
            json["system"].as_str(),
            Some("静态系统提示"),
            "top-level system 只该有开头的静态提示"
        );
        let wire = json["messages"].as_array().expect("messages");
        let found = wire.iter().find_map(|message| {
            if message["role"] != "user" {
                return None;
            }
            message["content"].as_array().and_then(|blocks| {
                blocks
                    .iter()
                    .find(|block| block["type"] == "text" && block["text"] == receipt.as_str())
            })
        });
        assert!(
            found.is_some(),
            "卡片回执要以 user 文本消息原样出现在历史里：{wire:?}"
        );
        assert_eq!(wire.last().unwrap()["role"], "user", "后面的用户消息还在末尾");
    }

    /// 开头连续的多个 system（系统提示带额外静态段）仍合并进 top-level system。
    #[test]
    fn leading_system_messages_still_merge_into_system() {
        let messages = vec![
            Message::system_prompt("系统提示"),
            Message::text(Role::System, "额外的静态规则"),
            Message::text(Role::User, "你好"),
        ];
        let json = body(&messages, &ToolRequest::default());
        assert_eq!(json["system"].as_str(), Some("系统提示\n\n额外的静态规则"));
        assert!(
            json["messages"]
                .as_array()
                .unwrap()
                .iter()
                .all(|message| message["role"] != "system"),
            "开头 system 不进 messages"
        );
    }

    /// tool_addition 的插入规则不变：仍紧跟 user/tool-result 回合，动态 system 回执不改变它。
    #[test]
    fn tool_addition_still_follows_user_or_tool_result() {
        let loaded = ToolSchema {
            name: "read".to_owned(),
            description: "读文件".to_owned(),
            parameters: serde_json::json!({"type": "object"}),
            defer_loading: true,
        };
        // after 用最后一条 user 消息的 id（构造后填真 id）。
        let user = Message::text(Role::User, "加载 read 工具");
        let after = user.id.clone();
        let history = ToolHistory {
            baseline: vec![],
            updates: vec![ToolAddition {
                after: Some(after),
                additions: vec![loaded.clone()],
            }],
        };
        let receipt = format!("{PREAMBLE} {CLOSING}\n卡片已接受");
        let messages = vec![
            Message::system_prompt("静态系统提示"),
            user,
            Message::assistant(
                vec![ContentBlock::text("好的")],
                "deepseek",
                "deepseek-flash",
            ),
            Message::text(Role::System, receipt.clone()),
            Message::text(Role::User, "继续"),
        ];
        let tools = ToolRequest {
            tools: vec![loaded],
            history: Some(history),
        };
        let json = body(&messages, &tools);
        let wire = json["messages"].as_array().expect("messages");
        // 插入点紧跟锚定的那条 user 消息；不能跑到动态 system 回执后面去。
        let anchor = wire
            .iter()
            .position(|message| {
                message["role"] == "user"
                    && message["content"]
                        .as_array()
                        .is_some_and(|blocks| blocks.iter().any(|b| b["text"] == "加载 read 工具"))
            })
            .expect("锚定 user 消息在 wire 里");
        let addition = &wire[anchor + 1];
        assert_eq!(addition["role"], "system", "tool_addition 还是 system 角色");
        assert_eq!(addition["content"][0]["type"], "tool_addition");
        assert_eq!(addition["content"][0]["tool"]["name"], "read");
        // 回执仍然在 user 消息里原文出现，没被 tool_addition 顶掉。
        let receipt_present = wire.iter().any(|message| {
            message["role"] == "user"
                && message["content"]
                    .as_array()
                    .is_some_and(|blocks| {
                        blocks
                            .iter()
                            .any(|block| block["text"] == receipt.as_str())
                    })
        });
        assert!(receipt_present, "回执原文要保留：{wire:?}");
    }
}
