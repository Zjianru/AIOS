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
mod image_projection;
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
pub use image_projection::project_images;
pub use ir::{
    BlockType, CanvasSource, ContentBlock, FinishReason, InstructionChange,
    InstructionChangeAction, Message, MessageId, MessageSource, QuoteSource, Role,
    SkillCatalogEntry, StreamChunk, ToolSchema, Usage, UserMessage,
};
pub use tool_updates::{ToolAddition, ToolHistory, ToolRequest, ToolUpdate};

/// D120：主模型看不了图时的一行占位；没有附件 id 时用旧原文。
pub fn image_omitted_text(attachment_ids: &[String]) -> String {
    if attachment_ids.is_empty() {
        return "(image omitted: model does not support images)".to_owned();
    }
    format!(
        "(image omitted: model does not support images; attachments: {})",
        attachment_ids.join(", ")
    )
}
