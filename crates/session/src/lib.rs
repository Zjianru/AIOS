//! 会话：只追加的事件日志、从日志派生模型消息、JSONL 存储。
//!
//! 机制照 DSH 的 packages/core/session 和 session-persistence-jsonl
//! （strategy 第 1 节）；格式是本仓库自己的 v2（spec 实现决定：事件和字段照 DSH）。

mod derive;
mod error;
mod event;
mod id;
mod lock;
mod projection;
mod repair;
mod store;

pub use error::SessionError;
pub use event::{
    AbortCause, Event, EventKind, FORMAT_VERSION, InboxOutcome, InboxTarget, JobToolCallData,
    LlmFailure, ModelSelection, RetryId, RetryMode, SessionHeader, ShadowedRange, SurfaceOp,
    TodoItem, TodoStatus, ToolError, TurnEndReason, last_model_selection, last_todos,
};
pub use id::SessionId;
pub use projection::{
    CanvasState, CanvasVersion, TODOS_PROJECTION_IN_SUMMARY, TODOS_PROJECTION_KEY,
    TODOS_PROJECTION_STATE_VERSION, TodosProjection, TodosProjectionView, canvas_versions,
    canvases, todos_projection,
};
pub use repair::{TurnClosers, interrupted_turn_closers};
pub use store::{AppendHook, AppendHookFactory, SessionSnapshot, SessionStore};

#[cfg(test)]
#[path = "format_tests.rs"]
mod format_tests;
#[cfg(test)]
#[path = "message_ir_tests.rs"]
mod message_ir;
#[cfg(test)]
#[path = "projection_tests.rs"]
mod projection_tests;
#[cfg(test)]
#[path = "repair_tests.rs"]
mod repair_tests;
#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;
