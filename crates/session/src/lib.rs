//! 会话：只追加的事件日志、从日志派生模型消息、JSONL 存储。
//!
//! 机制照 DSH 的 packages/core/session 和 session-persistence-jsonl
//! （strategy 第 1 节）；格式是本仓库自己的 v1（spec 实现决定）。

mod derive;
mod error;
mod event;
mod id;
mod repair;
mod store;

pub use derive::{COMPACTION_PREAMBLE, SUMMARY_CLOSE_TAG, SUMMARY_OPEN_TAG};
pub use error::SessionError;
pub use event::{Event, EventKind, SessionHeader, TodoItem, TodoStatus, TurnEndReason, last_todos};
pub use id::SessionId;
pub use repair::interrupted_turn_closers;
pub use store::SessionStore;
