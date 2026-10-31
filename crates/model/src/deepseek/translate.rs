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
            "error" => return Err(provider_error(&event, None)),
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
                    },
                );
                chunks.push(StreamChunk::BlockStart { index, block_type });
                if let Some(text) = initial_delta {
                    chunks.push(StreamChunk::TextDelta { index, text });
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
                match (delta_type, &mut block.content) {
                    ("text_delta", ContentBlock::Text { text }) => {
                        let piece = required_string(delta, "text")?;
                        text.push_str(&piece);
                        chunks.push(StreamChunk::TextDelta { index, text: piece });
                    }
                    ("input_json_delta", ContentBlock::ToolCall { .. }) => {
                        block
                            .json
                            .push_str(&required_string(delta, "partial_json")?);
                    }
                    (other, _) => return Err(malformed(&format!("不支持的内容增量 {other}"))),
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
                chunks.push(StreamChunk::Usage(self.usage.clone()));
                chunks.push(StreamChunk::Finish(finish));
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
