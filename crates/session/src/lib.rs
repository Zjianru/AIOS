//! 会话：只追加的事件日志、从日志派生模型消息、JSONL 存储。
//!
//! 机制照 DSH 的 packages/core/session 和 session-persistence-jsonl
//! （strategy 第 1 节）；格式是本仓库自己的 v2（spec 实现决定：事件和字段照 DSH）。

mod derive;
mod error;
mod event;
mod id;
mod repair;
mod store;

pub use derive::{COMPACTION_PREAMBLE, SUMMARY_CLOSE_TAG, SUMMARY_OPEN_TAG};
pub use error::SessionError;
pub use event::{
    AbortCause, Event, EventKind, InboxOutcome, InboxTarget, LlmFailure, SessionHeader, TodoItem,
    TodoStatus, ToolError, TurnEndReason, last_todos,
};
pub use id::SessionId;
pub use repair::{TurnClosers, interrupted_turn_closers};
pub use store::{SessionSnapshot, SessionStore};

#[cfg(test)]
#[path = "format_tests.rs"]
mod format_tests;
#[cfg(test)]
#[path = "message_ir_tests.rs"]
mod message_ir;
#[cfg(test)]
#[path = "repair_tests.rs"]
mod repair_tests;
#[cfg(test)]
#[path = "store_tests.rs"]
mod store_tests;
