//! 程序消息前缀表（唯一一份）：service 送进会话的程序消息只看 source.rpcId 前缀。
//!
//! 结果卡是用户消息里的 ContentBlock，不是程序消息；确认卡决定通知是 SystemMessage，
//! 两者都不在这张表里。

use model::{Message, MessageSource};

/// 唯一一张程序消息表。
pub const PROGRAM_MESSAGE_PREFIXES: [&str; 10] = [
    "task-report:",
    "task-stop:",
    "task-cc:",
    "gate-verdict:",
    "upstream-changed:",
    "appointment-wake:",
    "restart:",
    "fixed-block:",
    "date-now:",
    "dynamic-block:",
];

/// 需要按任务中继处理的前缀（工作页/做梦只看这四类）。
pub const TASK_RELAY_PREFIXES: [&str; 4] = [
    "task-report:",
    "task-stop:",
    "gate-verdict:",
    "upstream-changed:",
];

pub fn is_program_rpc(rpc_id: &str) -> bool {
    PROGRAM_MESSAGE_PREFIXES
        .iter()
        .any(|prefix| rpc_id.starts_with(prefix))
}

pub fn is_task_relay_rpc(rpc_id: &str) -> bool {
    TASK_RELAY_PREFIXES
        .iter()
        .any(|prefix| rpc_id.starts_with(prefix))
}

pub fn is_program_message(message: &Message) -> bool {
    matches!(
        &message.source,
        MessageSource::User { rpc_id: Some(rpc_id) } if is_program_rpc(rpc_id)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_exactly_the_ten_prefixes() {
        assert_eq!(
            PROGRAM_MESSAGE_PREFIXES,
            [
                "task-report:",
                "task-stop:",
                "task-cc:",
                "gate-verdict:",
                "upstream-changed:",
                "appointment-wake:",
                "restart:",
                "fixed-block:",
                "date-now:",
                "dynamic-block:",
            ]
        );
        assert!(is_program_rpc("task-report:t:r"));
        assert!(is_program_rpc("fixed-block:0123456789abcdef"));
        assert!(!is_program_rpc("confirm:card-1"));
    }

    #[test]
    fn fixed_block_is_a_program_message_not_a_user_bubble() {
        let message =
            Message::user_text_with_request_id("页面清单", "fixed-block:0123456789abcdef");
        assert!(is_program_message(&message));
        let user = Message::user_text_with_request_id("普通用户话", "user-rpc-1");
        assert!(!is_program_message(&user));
    }
}
