use std::collections::{HashMap, HashSet};

use model::{AssistantStreamRecord, BlockType, ContentBlock, FinishReason, StreamChunk, Usage};
use serde_json::{Value, json};

/// 把紧凑流记录编码成 Messages SSE 帧（完整流：带 message_delta + message_stop）。
pub fn encode_sse(records: &[AssistantStreamRecord]) -> Vec<String> {
    encode(records, true)
}

/// 编码一段流后把连接断掉（没有 message_delta/message_stop；模拟流到一半断开）。
pub fn encode_sse_cut(records: &[AssistantStreamRecord]) -> Vec<String> {
    encode(records, false)
}

fn encode(records: &[AssistantStreamRecord], complete: bool) -> Vec<String> {
    let mut frames = Vec::new();
    let mut started: HashSet<usize> = HashSet::new();
    let mut kinds: HashMap<usize, BlockType> = HashMap::new();
    let mut usage: Option<Usage> = None;
    let mut finished = false;
    frames.push(frame(
        "message_start",
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_replay",
                "type": "message",
                "role": "assistant",
                "model": "replay",
                "content": [],
                "usage": {}
            }
        }),
    ));
    for record in records {
        match record {
            AssistantStreamRecord::TextChunks { index, texts, .. } => {
                kinds.insert(*index, BlockType::Text);
                if started.insert(*index) {
                    frames.push(start_text(*index));
                }
                for text in texts {
                    frames.push(frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "text_delta", "text": text}
                        }),
                    ));
                }
            }
            AssistantStreamRecord::ReasoningChunks { index, texts, .. } => {
                kinds.insert(*index, BlockType::Reasoning);
                if started.insert(*index) {
                    frames.push(start_reasoning(*index));
                }
                for text in texts {
                    frames.push(frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "thinking_delta", "thinking": text}
                        }),
                    ));
                }
            }
            AssistantStreamRecord::ToolCallChunks {
                index,
                id,
                name,
                args,
                ..
            } => {
                kinds.insert(*index, BlockType::ToolCall);
                if started.insert(*index) {
                    frames.push(start_tool(*index, id, name.as_deref().unwrap_or(id)));
                }
                for arg in args {
                    if arg.is_empty() {
                        continue;
                    }
                    frames.push(frame(
                        "content_block_delta",
                        json!({
                            "type": "content_block_delta",
                            "index": index,
                            "delta": {"type": "input_json_delta", "partial_json": arg}
                        }),
                    ));
                }
            }
            AssistantStreamRecord::Chunk { chunk, .. } => match chunk {
                StreamChunk::BlockStart { index, block_type } => {
                    kinds.insert(*index, *block_type);
                    match block_type {
                        BlockType::Text => {
                            if started.insert(*index) {
                                frames.push(start_text(*index));
                            }
                        }
                        BlockType::Reasoning => {
                            if started.insert(*index) {
                                frames.push(start_reasoning(*index));
                            }
                        }
                        BlockType::ToolCall => {}
                    }
                }
                StreamChunk::BlockEnd { index, block } => {
                    if !started.contains(index) {
                        match block {
                            ContentBlock::Text { text, .. } => {
                                frames.push(start_text_with(*index, text));
                                started.insert(*index);
                            }
                            ContentBlock::Reasoning { text, signature } => {
                                frames.push(start_reasoning_with(*index, text));
                                started.insert(*index);
                                if let Some(signature) = signature {
                                    frames.push(frame(
                                        "content_block_delta",
                                        json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {"type": "signature_delta", "signature": signature}
                                        }),
                                    ));
                                }
                            }
                            ContentBlock::ToolCall {
                                id,
                                name,
                                arguments,
                            } => {
                                frames.push(start_tool(*index, id, name));
                                started.insert(*index);
                                if arguments != "{}" {
                                    frames.push(frame(
                                        "content_block_delta",
                                        json!({
                                            "type": "content_block_delta",
                                            "index": index,
                                            "delta": {"type": "input_json_delta", "partial_json": arguments}
                                        }),
                                    ));
                                }
                            }
                            // 文件块只在用户消息里，assistant 流里不会出现。
                            ContentBlock::File { .. } => {}
                        }
                    } else if let ContentBlock::Reasoning {
                        signature: Some(signature),
                        ..
                    } = block
                    {
                        frames.push(frame(
                            "content_block_delta",
                            json!({
                                "type": "content_block_delta",
                                "index": index,
                                "delta": {"type": "signature_delta", "signature": signature}
                            }),
                        ));
                    }
                    frames.push(frame(
                        "content_block_stop",
                        json!({"type": "content_block_stop", "index": index}),
                    ));
                }
                StreamChunk::Usage { usage: reported } => usage = Some(reported.clone()),
                StreamChunk::Finish { reason } if complete => {
                    frames.push(frame(
                        "message_delta",
                        json!({
                            "type": "message_delta",
                            "delta": {"stop_reason": stop_reason(*reason)},
                            "usage": usage.as_ref().map(usage_wire).unwrap_or(json!({}))
                        }),
                    ));
                    frames.push(frame("message_stop", json!({"type": "message_stop"})));
                    finished = true;
                }
                _ => {}
            },
        }
        if finished {
            break;
        }
    }
    if complete && !finished {
        frames.push(frame(
            "message_delta",
            json!({
                "type": "message_delta",
                "delta": {"stop_reason": "end_turn"},
                "usage": usage.as_ref().map(usage_wire).unwrap_or(json!({}))
            }),
        ));
        frames.push(frame("message_stop", json!({"type": "message_stop"})));
    }
    frames
}

fn frame(event: &str, data: Value) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn start_text(index: usize) -> String {
    frame(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "text", "text": ""}
        }),
    )
}

fn start_text_with(index: usize, text: &str) -> String {
    frame(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "text", "text": text}
        }),
    )
}

fn start_reasoning(index: usize) -> String {
    frame(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "thinking", "thinking": ""}
        }),
    )
}

fn start_reasoning_with(index: usize, text: &str) -> String {
    frame(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "thinking", "thinking": text}
        }),
    )
}

fn start_tool(index: usize, id: &str, name: &str) -> String {
    frame(
        "content_block_start",
        json!({
            "type": "content_block_start",
            "index": index,
            "content_block": {"type": "tool_use", "id": id, "name": name, "input": {}}
        }),
    )
}

fn stop_reason(reason: FinishReason) -> &'static str {
    match reason {
        FinishReason::Stop => "end_turn",
        FinishReason::ToolCalls => "tool_use",
        FinishReason::MaxTokens => "max_tokens",
    }
}

fn usage_wire(usage: &Usage) -> Value {
    let mut map = serde_json::Map::new();
    if let Some(value) = usage.input_tokens {
        map.insert("input_tokens".to_owned(), json!(value));
    }
    if let Some(value) = usage.output_tokens {
        map.insert("output_tokens".to_owned(), json!(value));
    }
    if let Some(value) = usage.cache_read_input_tokens {
        map.insert("cache_read_input_tokens".to_owned(), json!(value));
    }
    if let Some(value) = usage.cache_creation_input_tokens {
        map.insert("cache_creation_input_tokens".to_owned(), json!(value));
    }
    Value::Object(map)
}
