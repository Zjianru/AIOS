//! 一次模型尝试的精确保存流（照 DSH `packages/llm/llm/src/assistant-stream.ts`）。
//!
//! DSH 把逐帧 chunk 和时间压成紧凑记录写进 `assistant/message` / `assistant/attempt` 的
//! `stream` 字段：连续的 text/reasoning delta 合成一条 run（`time0` + 相邻 `dt` 间隔 +
//! 原始 `texts`），其余 chunk 保留成 `{type:'chunk', time, chunk}`。展开后逐帧和原始流
//! 完全一致（delta 边界不合并不丢）。

use serde::{Deserialize, Serialize};

use crate::StreamChunk;

/// 一帧模型流和它到达时的会话时间（Unix ms；照 DSH TimedStreamChunk）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimedStreamChunk {
    pub time: u64,
    pub chunk: StreamChunk,
}

/// `assistant/message` / `assistant/attempt` 里保存的紧凑流记录。
///
/// 记录形状照 DSH（assistant-stream.ts:20-46）；T006.14 起适配器也发 tool-call-delta
/// （translate.ts:82-85,130），参数增量合成 `tool-call-chunks` 记录。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AssistantStreamRecord {
    /// 一段连续的 text delta：`time0` 是第一帧时间，`dt[i]` 是第 i+1 帧相对前一帧的间隔。
    TextChunks {
        time0: u64,
        index: usize,
        dt: Vec<u64>,
        texts: Vec<String>,
    },
    /// 一段连续的 reasoning delta（字段同 text-chunks）。
    ReasoningChunks {
        time0: u64,
        index: usize,
        dt: Vec<u64>,
        texts: Vec<String>,
    },
    /// 一段连续的 tool-call delta（字段同 text-chunks，另带工具身份；照 DSH
    /// assistant-stream.ts:35-43 的 tool-call-chunks）。
    ToolCallChunks {
        time0: u64,
        index: usize,
        dt: Vec<u64>,
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        args: Vec<String>,
    },
    /// 其余 chunk（block-start / block-end / usage / finish）原样保留。
    Chunk { time: u64, chunk: StreamChunk },
}

/// 压缩中的连续 run 类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RunKind {
    Text,
    Reasoning,
    ToolCall,
}

/// tool-call run 的工具身份：id 必填、name 只在起始帧带（DSH 的 sameName 判定）。
#[derive(Debug, Clone, PartialEq, Eq)]
struct ToolIdentity {
    id: String,
    name: Option<String>,
}

/// 当前还能继续追加的 run。
#[derive(Debug, Clone)]
struct OpenRun {
    kind: RunKind,
    index: usize,
    last_time: u64,
    tool: Option<ToolIdentity>,
}

/// 把一帧帧流压成 `AssistantStreamRecord`；`snapshot` 是写进日志的稳定值。
#[derive(Debug, Default, Clone)]
pub struct AssistantStreamAccumulator {
    records: Vec<AssistantStreamRecord>,
    open: Option<OpenRun>,
}

impl AssistantStreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加入一帧（time 是会话时间；同 DSH 的 push）。
    pub fn push(&mut self, timed: TimedStreamChunk) {
        match &timed.chunk {
            StreamChunk::TextDelta { index, text } => {
                self.push_delta(RunKind::Text, *index, text, timed.time)
            }
            StreamChunk::ReasoningDelta { index, text } => {
                self.push_delta(RunKind::Reasoning, *index, text, timed.time)
            }
            StreamChunk::ToolCallDelta {
                index,
                id,
                name,
                arguments_delta,
            } => self.push_tool_delta(*index, id, name.as_deref(), arguments_delta, timed.time),
            _ => {
                self.open = None;
                self.records.push(AssistantStreamRecord::Chunk {
                    time: timed.time,
                    chunk: timed.chunk,
                });
            }
        }
    }

    /// 当前记录的快照（深拷贝；日志/事件用）。
    pub fn snapshot(&self) -> Vec<AssistantStreamRecord> {
        self.records.clone()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    /// 连续同类型同 index 且时间不倒退的 delta 追加进最后一条 run，否则新开一条。
    fn push_delta(&mut self, kind: RunKind, index: usize, text: &str, time: u64) {
        if let Some(open) = self.open.clone()
            && open.kind == kind
            && open.index == index
            && time >= open.last_time
        {
            let appended = match (kind, self.records.last_mut()) {
                (RunKind::Text, Some(AssistantStreamRecord::TextChunks { dt, texts, .. })) => {
                    dt.push(time - open.last_time);
                    texts.push(text.to_owned());
                    true
                }
                (
                    RunKind::Reasoning,
                    Some(AssistantStreamRecord::ReasoningChunks { dt, texts, .. }),
                ) => {
                    dt.push(time - open.last_time);
                    texts.push(text.to_owned());
                    true
                }
                _ => false,
            };
            if appended {
                self.open = Some(OpenRun {
                    last_time: time,
                    ..open
                });
                return;
            }
        }
        let record = match kind {
            RunKind::Text => AssistantStreamRecord::TextChunks {
                time0: time,
                index,
                dt: Vec::new(),
                texts: vec![text.to_owned()],
            },
            RunKind::Reasoning => AssistantStreamRecord::ReasoningChunks {
                time0: time,
                index,
                dt: Vec::new(),
                texts: vec![text.to_owned()],
            },
            // tool-call delta 只走 push_tool_delta（调用点已分派）。
            RunKind::ToolCall => return,
        };
        self.records.push(record);
        self.open = Some(OpenRun {
            kind,
            index,
            last_time: time,
            tool: None,
        });
    }

    /// tool-call delta 的压缩（照 DSH assistant-stream.ts:129-166）：同一工具、同 index、
    /// 时间不倒退且 name 的带/不带状态一致才追加；起始终止帧（带 name）之后是纯参数增量。
    /// 空 id 或空 name 的帧按 DSH 不压缩、原样落 chunk。
    fn push_tool_delta(
        &mut self,
        index: usize,
        id: &str,
        name: Option<&str>,
        args: &str,
        time: u64,
    ) {
        if id.is_empty() || name == Some("") {
            self.open = None;
            self.records.push(AssistantStreamRecord::Chunk {
                time,
                chunk: StreamChunk::ToolCallDelta {
                    index,
                    id: id.to_owned(),
                    name: name.map(str::to_owned),
                    arguments_delta: args.to_owned(),
                },
            });
            return;
        }
        let identity = ToolIdentity {
            id: id.to_owned(),
            name: name.map(str::to_owned),
        };
        if let Some(open) = self.open.clone()
            && open.kind == RunKind::ToolCall
            && open.index == index
            && time >= open.last_time
            && open.tool.as_ref() == Some(&identity)
            && let Some(AssistantStreamRecord::ToolCallChunks {
                dt,
                args: fragments,
                ..
            }) = self.records.last_mut()
        {
            dt.push(time - open.last_time);
            fragments.push(args.to_owned());
            self.open = Some(OpenRun {
                last_time: time,
                ..open
            });
            return;
        }
        self.records.push(AssistantStreamRecord::ToolCallChunks {
            time0: time,
            index,
            dt: Vec::new(),
            id: identity.id.clone(),
            name: identity.name.clone(),
            args: vec![args.to_owned()],
        });
        self.open = Some(OpenRun {
            kind: RunKind::ToolCall,
            index,
            last_time: time,
            tool: Some(identity),
        });
    }
}

/// 把紧凑记录展开回逐帧 chunk（照 DSH expandAssistantStream；delta 边界原样保留）。
pub fn expand_assistant_stream(stream: &[AssistantStreamRecord]) -> Vec<TimedStreamChunk> {
    let mut chunks = Vec::new();
    for record in stream {
        match record {
            AssistantStreamRecord::Chunk { time, chunk } => {
                chunks.push(TimedStreamChunk {
                    time: *time,
                    chunk: chunk.clone(),
                });
            }
            AssistantStreamRecord::TextChunks {
                time0,
                index,
                dt,
                texts,
            }
            | AssistantStreamRecord::ReasoningChunks {
                time0,
                index,
                dt,
                texts,
            } => {
                let reasoning = matches!(record, AssistantStreamRecord::ReasoningChunks { .. });
                let mut time = *time0;
                for (position, text) in texts.iter().enumerate() {
                    if position > 0 {
                        time = time.saturating_add(dt[position - 1]);
                    }
                    let chunk = if reasoning {
                        StreamChunk::ReasoningDelta {
                            index: *index,
                            text: text.clone(),
                        }
                    } else {
                        StreamChunk::TextDelta {
                            index: *index,
                            text: text.clone(),
                        }
                    };
                    chunks.push(TimedStreamChunk { time, chunk });
                }
            }
            AssistantStreamRecord::ToolCallChunks {
                time0,
                index,
                dt,
                id,
                name,
                args,
            } => {
                let mut time = *time0;
                for (position, arguments) in args.iter().enumerate() {
                    if position > 0 {
                        time = time.saturating_add(dt[position - 1]);
                    }
                    chunks.push(TimedStreamChunk {
                        time,
                        chunk: StreamChunk::ToolCallDelta {
                            index: *index,
                            id: id.clone(),
                            name: name.clone(),
                            arguments_delta: arguments.clone(),
                        },
                    });
                }
            }
        }
    }
    chunks
}
