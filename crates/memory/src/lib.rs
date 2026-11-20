//! 记忆库：关于你、条目、Kind 的笔记和日记、做梦记录、隔离区、FTS5 搜索。
//!
//! 边界：不依赖 service/agent/tasks/session；输入是纯文本与来源引用。

mod error;
mod store;
pub mod types;

pub use error::{DreamValidationError, MemoryError, WriteError};
pub use store::{MemoryStore, SCHEMA_VERSION};
