//! 看图的会话旁注与全局描述。
//!
//! 会话旁注冻结「这张图第一次发给模型时的那段文字」；全局描述按内容 id 一张一个
//! 文件，只用于第一次发送时选文字。两者都不改会话日志，也不进长期记忆。

mod notes;
pub mod ops;
mod settings;
mod text;

pub use notes::{GlobalNote, GlobalNotes, NoteKind, SessionNotes, VisionNote};
pub use settings::{VisionModelRef, VisionSettings};
pub use text::{DESCRIPTION_LIMIT, backend_failed_text, described_text, omitted_text};
