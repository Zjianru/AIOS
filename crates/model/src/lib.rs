//! 模型层：provider 无关的消息 IR，以及 DeepSeek 的 Messages 适配器。
//!
//! 照抄来源：DSH 的 packages/llm 与 packages/llm/llm-deepseek（strategy 第 1 节）。

mod assistant_stream;
mod cancel;
mod deepseek;
mod error;
mod ir;

pub use assistant_stream::{
    AssistantStreamAccumulator, AssistantStreamRecord, TimedStreamChunk, expand_assistant_stream,
};
pub use cancel::CancelToken;
pub use deepseek::{
    ChunkStream, DEFAULT_BASE_URL, DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_TOKENS, DeepSeek,
    DeepSeekConfig,
};
pub use error::{ErrorCode, ModelError};
pub use ir::{
    BlockType, ContentBlock, Effort, FinishReason, InstructionChange, InstructionChangeAction,
    Message, MessageId, MessageSource, Role, SkillCatalogEntry, StreamChunk, ToolSchema, Usage,
    UserMessage,
};
