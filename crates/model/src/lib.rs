//! 模型层：provider 无关的消息 IR，以及 DeepSeek 的 Messages 适配器。
//!
//! 照抄来源：DSH 的 packages/llm 与 packages/llm/llm-deepseek（strategy 第 1 节）。

mod anthropic;
mod assistant_stream;
mod cancel;
pub mod catalog;
mod credential;
mod effort;
mod error;
pub mod http;
mod ir;
mod tool_updates;

pub use anthropic::actual_effort;
pub use anthropic::file_handle_text;
pub use anthropic::{
    ChunkStream, DEFAULT_BASE_URL, DEFAULT_CONTEXT_WINDOW, DEFAULT_MAX_TOKENS, DeepSeek,
    DeepSeekConfig, default_effort_for,
};
pub use assistant_stream::{
    AssistantStreamAccumulator, AssistantStreamRecord, TimedStreamChunk, expand_assistant_stream,
};
pub use cancel::CancelToken;
pub use credential::{CredentialContext, CredentialSource};
pub use effort::{
    EFFORT_ALIASES, EFFORT_RANK, Effort, budget_of, default_effort, effort_level, fit_effort,
    nearest_effort, output_effort_at,
};
pub use error::{ErrorCode, ModelError};
pub use ir::{
    BlockType, CanvasSource, ContentBlock, FinishReason, InstructionChange,
    InstructionChangeAction, Message, MessageId, MessageSource, Role, SkillCatalogEntry,
    StreamChunk, ToolSchema, Usage, UserMessage,
};
pub use tool_updates::{ToolAddition, ToolHistory, ToolRequest, ToolUpdate};
