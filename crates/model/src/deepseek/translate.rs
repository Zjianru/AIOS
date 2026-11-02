//! 把 Messages 的 wire 事件翻译成 provider 无关的流事件。

use std::collections::BTreeMap;

use serde_json::Value;

use crate::{BlockType, ContentBlock, ErrorCode, FinishReason, ModelError, StreamChunk, Usage};

use super::transport::provider_error;

struct BlockState {
    content: ContentBlock,
    closed: bool,
    /// tool_use 块累计的 input JSON（input_json_delta 拼接）。
    json: String,
    /// thinking 块累计的 signature（signature_delta 拼接，块结束时放进 Reasoning）。
    signature: Option<String>,
}

/// 一次响应的翻译状态机：保留块顺序，累计用量，产出一个最终结束原因。
pub(super) struct Translator {
    started: bool,
    blocks: BTreeMap<usize, BlockState>,
    usage: Usage,
    finish: Option<FinishReason>,
    finished: bool,
}

impl Translator {
    pub(super) fn new() -> Self {
        Self {
            started: false,
            blocks: BTreeMap::new(),
            usage: Usage::default(),
            finish: None,
            finished: false,
        }
    }

    /// 流是否已经结算（收到 message_stop）。
    pub(super) fn is_finished(&self) -> bool {
        self.finished
    }

    /// 喂一个 wire 事件，返回这次新增的流事件。
    pub(super) fn push(&mut self, event: Value) -> Result<Vec<StreamChunk>, ModelError> {
        if self.finished {
            return Ok(Vec::new());
        }
        let mut chunks = Vec::new();
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("事件缺少 type"))?
            .to_owned();
        match kind.as_str() {
            "error" => return Err(provider_error(&event, None, None)),
            "message_start" => {
                if self.started {
                    return Err(malformed("重复的 message_start"));
                }
                if let Some(message) = event.get("message") {
                    update_usage(&mut self.usage, message.get("usage"))?;
                }
                self.started = true;
            }
            "content_block_start" => {
                self.require_started()?;
                let index = block_index(&event)?;
                if self.blocks.contains_key(&index) || self.finish.is_some() {
                    return Err(malformed("content_block_start 的位置非法或重复"));
                }
                let native = event
                    .get("content_block")
                    .ok_or_else(|| malformed("content_block_start 缺少 content_block"))?;
                let native_type = native
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("内容块缺少 type"))?;
                let (content, block_type, initial_delta) = match native_type {
                    "thinking" => {
                        let text = required_string(native, "thinking")?;
                        let initial = if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        };
                        (
                            ContentBlock::Reasoning {
                                text,
                                signature: None,
                            },
                            BlockType::Reasoning,
                            initial,
                        )
                    }
                    "text" => {
                        let text = required_string(native, "text")?;
                        let initial = if text.is_empty() {
                            None
                        } else {
                            Some(text.clone())
                        };
                        (ContentBlock::Text { text }, BlockType::Text, initial)
                    }
                    "tool_use" => {
                        let id = required_string(native, "id")?;
                        let name = required_string(native, "name")?;
                        if id.is_empty() || name.is_empty() {
                            return Err(malformed("空的工具调用身份"));
                        }
                        let input = native
                            .get("input")
                            .filter(|value| value.is_object())
                            .cloned()
                            .unwrap_or_else(|| Value::Object(serde_json::Map::new()));
                        let arguments = serde_json::to_string(&input)
                            .map_err(|_| malformed("工具调用的 input 不能序列化"))?;
                        (
                            ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            },
                            BlockType::ToolCall,
                            None,
                        )
                    }
                    other => {
                        return Err(ModelError::new(
                            ErrorCode::UnsupportedContent,
                            format!("DeepSeek Messages 响应块 {other} 当前不支持"),
                        ));
                    }
                };
                self.blocks.insert(
                    index,
                    BlockState {
                        content,
                        closed: false,
                        json: String::new(),
                        signature: None,
                    },
                );
                chunks.push(StreamChunk::BlockStart { index, block_type });
                if let Some(text) = initial_delta {
                    match block_type {
                        BlockType::Text => chunks.push(StreamChunk::TextDelta { index, text }),
                        BlockType::Reasoning => {
                            chunks.push(StreamChunk::ReasoningDelta { index, text });
                        }
                        BlockType::ToolCall => {}
                    }
                }
            }
            "content_block_delta" => {
                self.require_started()?;
                let index = block_index(&event)?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| malformed("增量事件指向未打开的内容块"))?;
                if block.closed {
                    return Err(malformed("内容块关闭后仍有增量"));
                }
                let delta = event
                    .get("delta")
                    .ok_or_else(|| malformed("content_block_delta 缺少 delta"))?;
                let delta_type = delta
                    .get("type")
                    .and_then(Value::as_str)
                    .ok_or_else(|| malformed("delta 缺少 type"))?;
                match delta_type {
                    "text_delta" => {
                        if let ContentBlock::Text { text } = &mut block.content {
                            let piece = required_string(delta, "text")?;
                            text.push_str(&piece);
                            chunks.push(StreamChunk::TextDelta { index, text: piece });
                        } else {
                            return Err(malformed("text_delta 指向非文本块"));
                        }
                    }
                    "thinking_delta" => {
                        if let ContentBlock::Reasoning { text, .. } = &mut block.content {
                            let piece = required_string(delta, "thinking")?;
                            text.push_str(&piece);
                            chunks.push(StreamChunk::ReasoningDelta { index, text: piece });
                        } else {
                            return Err(malformed("thinking_delta 指向非 thinking 块"));
                        }
                    }
                    "signature_delta" => {
                        if let ContentBlock::Reasoning { .. } = &block.content {
                            let piece = required_string(delta, "signature")?;
                            block
                                .signature
                                .get_or_insert_with(String::new)
                                .push_str(&piece);
                        } else {
                            return Err(malformed("signature_delta 指向非 thinking 块"));
                        }
                    }
                    "input_json_delta" => {
                        if let ContentBlock::ToolCall { .. } = &mut block.content {
                            block
                                .json
                                .push_str(&required_string(delta, "partial_json")?);
                        } else {
                            return Err(malformed("input_json_delta 指向非工具调用块"));
                        }
                    }
                    other => return Err(malformed(&format!("不支持的内容增量 {other}"))),
                }
            }
            "content_block_stop" => {
                self.require_started()?;
                let index = block_index(&event)?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| malformed("stop 事件指向未打开的内容块"))?;
                if block.closed {
                    return Err(malformed("内容块重复关闭"));
                }
                block.closed = true;
                if let ContentBlock::ToolCall { arguments, .. } = &mut block.content
                    && !block.json.is_empty()
                {
                    *arguments = std::mem::take(&mut block.json);
                }
                if let ContentBlock::Reasoning { signature, .. } = &mut block.content
                    && block.signature.is_some()
                {
                    *signature = block.signature.clone();
                }
                let block = block.content.clone();
                chunks.push(StreamChunk::BlockEnd { index, block });
            }
            "message_delta" => {
                self.require_started()?;
                if let Some(delta) = event.get("delta")
                    && let Some(raw) = delta.get("stop_reason")
                    && !raw.is_null()
                {
                    self.finish = Some(stop_reason(raw)?);
                }
                update_usage(&mut self.usage, event.get("usage"))?;
            }
            "message_stop" => {
                self.require_started()?;
                let Some(finish) = self.finish else {
                    return Err(malformed("message_stop 之前没有 stop_reason"));
                };
                if self.blocks.values().any(|block| !block.closed) {
                    return Err(malformed("message_stop 时仍有未关闭的内容块"));
                }
                if self.blocks.is_empty() && finish == FinishReason::Stop {
                    return Err(ModelError::new(
                        ErrorCode::EmptyResponse,
                        "DeepSeek Messages 返回了空响应",
                    ));
                }
                // max-tokens 截断时工具参数可能不完整，由调用方丢弃；其余情况要求合法 JSON 对象。
                if finish != FinishReason::MaxTokens {
                    for block in self.blocks.values() {
                        let ContentBlock::ToolCall { arguments, .. } = &block.content else {
                            continue;
                        };
                        let parsed: Value = serde_json::from_str(arguments)
                            .map_err(|_| malformed("工具输入的 JSON 非法"))?;
                        if !parsed.is_object() {
                            return Err(malformed("工具输入不是 JSON 对象"));
                        }
                    }
                }
                self.finished = true;
                // K10：整段 usage 缺失时不出 usage 帧（不伪造 0/空用量；DSH 的适配器只在
                // 真的报过用量时才带 usage）。
                if !self.usage.is_empty() {
                    chunks.push(StreamChunk::Usage {
                        usage: self.usage.clone(),
                    });
                }
                chunks.push(StreamChunk::Finish { reason: finish });
            }
            // Anthropic 允许其他事件类型（如 ping）：忽略。
            _ => {}
        }
        Ok(chunks)
    }

    fn require_started(&self) -> Result<(), ModelError> {
        if self.started {
            Ok(())
        } else {
            Err(malformed("内容事件早于 message_start"))
        }
    }
}

fn malformed(detail: &str) -> ModelError {
    ModelError::new(
        ErrorCode::MalformedResponse,
        format!("DeepSeek Messages stream: {detail}"),
    )
}

fn block_index(event: &Value) -> Result<usize, ModelError> {
    event
        .get("index")
        .and_then(Value::as_u64)
        .map(|index| index as usize)
        .ok_or_else(|| malformed("事件缺少合法的 index"))
}

fn required_string(value: &Value, field: &str) -> Result<String, ModelError> {
    value
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| malformed(&format!("缺少字符串字段 {field}")))
}

fn stop_reason(raw: &Value) -> Result<FinishReason, ModelError> {
    match raw.as_str() {
        Some("end_turn" | "stop_sequence") => Ok(FinishReason::Stop),
        Some("tool_use") => Ok(FinishReason::ToolCalls),
        Some("max_tokens") => Ok(FinishReason::MaxTokens),
        _ => Err(malformed("不支持的 stop_reason")),
    }
}

fn update_usage(usage: &mut Usage, raw: Option<&Value>) -> Result<(), ModelError> {
    let Some(raw) = raw else {
        return Ok(());
    };
    if raw.is_null() {
        return Ok(());
    }
    let Some(object) = raw.as_object() else {
        return Err(malformed("usage 不是对象"));
    };
    let fields: [(&str, &mut Option<u64>); 4] = [
        ("input_tokens", &mut usage.input_tokens),
        ("output_tokens", &mut usage.output_tokens),
        (
            "cache_read_input_tokens",
            &mut usage.cache_read_input_tokens,
        ),
        (
            "cache_creation_input_tokens",
            &mut usage.cache_creation_input_tokens,
        ),
    ];
    for (wire, target) in fields {
        if let Some(value) = object.get(wire) {
            let tokens = value
                .as_u64()
                .ok_or_else(|| malformed(&format!("usage.{wire} 不是非负整数")))?;
            *target = Some(tokens);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn push(translator: &mut Translator, event: Value) -> Vec<StreamChunk> {
        translator.push(event).expect("合法 wire 事件")
    }

    /// 空响应归 EMPTY_RESPONSE（K10 点名项）。
    #[test]
    fn empty_stream_maps_to_empty_response() {
        let mut translator = Translator::new();
        push(
            &mut translator,
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 1}}}),
        );
        push(
            &mut translator,
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        );
        let error = translator
            .push(json!({"type": "message_stop"}))
            .expect_err("没有内容块应报错");
        assert_eq!(error.code, ErrorCode::EmptyResponse);
    }

    /// K10：整段 usage 缺失时不产生 usage 帧（不伪造 0/空用量）。
    #[test]
    fn usage_missing_produces_no_usage_chunk() {
        let mut translator = Translator::new();
        let mut chunks = push(
            &mut translator,
            json!({"type": "message_start", "message": {"content": []}}),
        );
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}}),
        ));
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "好"}}),
        ));
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_stop", "index": 0}),
        ));
        chunks.extend(push(
            &mut translator,
            json!({"type": "message_delta", "delta": {"stop_reason": "end_turn"}}),
        ));
        chunks.extend(push(&mut translator, json!({"type": "message_stop"})));
        assert!(
            !chunks
                .iter()
                .any(|chunk| matches!(chunk, StreamChunk::Usage { .. })),
            "usage 整段缺失时不能伪造 usage 帧：{chunks:?}"
        );
    }

    /// T006.14（DSH translate.ts:82-85,130）：工具参数增量逐帧发 tool-call-delta；
    /// block-start 时带 id/name、argumentsDelta 为空，之后每段 input_json_delta 一帧。
    #[test]
    fn tool_call_deltas_match_dsh() {
        let mut translator = Translator::new();
        let mut chunks = push(
            &mut translator,
            json!({"type": "message_start", "message": {"usage": {"input_tokens": 1}}}),
        );
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_start", "index": 0, "content_block": {"type": "tool_use", "id": "call_0", "name": "read", "input": {}}}),
        ));
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "{\"file\""}}),
        ));
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_delta", "index": 0, "delta": {"type": "input_json_delta", "partial_json": "_path\":\"a\"}"}}),
        ));
        chunks.extend(push(
            &mut translator,
            json!({"type": "content_block_stop", "index": 0}),
        ));
        assert_eq!(
            chunks,
            vec![
                StreamChunk::BlockStart {
                    index: 0,
                    block_type: BlockType::ToolCall,
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: "call_0".to_owned(),
                    name: Some("read".to_owned()),
                    arguments_delta: String::new(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: "call_0".to_owned(),
                    name: None,
                    arguments_delta: "{\"file\"".to_owned(),
                },
                StreamChunk::ToolCallDelta {
                    index: 0,
                    id: "call_0".to_owned(),
                    name: None,
                    arguments_delta: "_path\":\"a\"}".to_owned(),
                },
                StreamChunk::BlockEnd {
                    index: 0,
                    block: ContentBlock::ToolCall {
                        id: "call_0".to_owned(),
                        name: "read".to_owned(),
                        arguments: "{\"file_path\":\"a\"}".to_owned(),
                    },
                },
            ]
        );
    }
}
