//! service 用的程序消息判断：前缀表的唯一一份在 agent crate，这里只转发。
//!
//! 结果卡是用户消息里的 ContentBlock，不是程序消息；确认卡决定通知是 SystemMessage，
//! 两者都不在这张表里。

pub(crate) use agent::{is_program_message, is_program_rpc, is_task_relay_rpc};
