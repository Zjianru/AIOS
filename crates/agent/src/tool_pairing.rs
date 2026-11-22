//! 组模型请求前的工具配对消毒：日志里可能有被取消的问题留下的孤儿 tool call。
//!
//! 只改发给模型的内存消息，不改会话日志/EventKind/协议。

use std::collections::HashSet;

use model::{ContentBlock, Message, Role};

/// 丢掉没有对应 tool/result 的 assistant tool call，以及没有对应 tool call 的 tool result；
/// assistant 的文本块原样保留。正常配对的消息不动。
pub(crate) fn sanitize(messages: Vec<Message>) -> Vec<Message> {
    let mut calls: HashSet<String> = HashSet::new();
    let mut results: HashSet<String> = HashSet::new();
    for message in &messages {
        if message.role == Role::Assistant {
            for block in &message.content {
                if let ContentBlock::ToolCall { id, .. } = block {
                    calls.insert(id.clone());
                }
            }
        }
        if let Some(call_id) = &message.tool_call_id {
            results.insert(call_id.clone());
        }
    }

    let mut clean = Vec::with_capacity(messages.len());
    for message in messages {
        match message.role {
            Role::Assistant => {
                let mut message = message;
                let source = std::mem::take(&mut message.content);
                let mut content = Vec::with_capacity(source.len());
                for block in source {
                    match &block {
                        ContentBlock::ToolCall { id, .. } if !results.contains(id) => {}
                        _ => content.push(block),
                    }
                }
                if content.is_empty() {
                    continue;
                }
                message.content = content;
                clean.push(message);
            }
            Role::Tool => {
                let Some(call_id) = message.tool_call_id.as_deref() else {
                    continue;
                };
                if calls.contains(call_id) {
                    clean.push(message);
                }
            }
            _ => clean.push(message),
        }
    }
    clean
}

#[cfg(test)]
mod tests {
    use super::*;
    use model::{ContentBlock, Message};

    fn text(role: Role, text: &str) -> Message {
        Message::text(role, text)
    }

    fn call(id: &str) -> ContentBlock {
        ContentBlock::ToolCall {
            id: id.to_owned(),
            name: "ask_user_question".to_owned(),
            arguments: "{}".to_owned(),
        }
    }

    #[test]
    fn orphan_question_call_is_dropped_but_assistant_text_kept() {
        let messages = vec![
            text(Role::User, "开始"),
            {
                let mut message = Message::assistant(
                    vec![ContentBlock::text("我先问一句"), call("orphan")],
                    "deepseek",
                    "deepseek-flash",
                );
                message.id = model::MessageId::generate();
                message
            },
            text(Role::User, "取消后继续"),
        ];
        let clean = sanitize(messages);
        assert_eq!(clean.len(), 3, "assistant 文本还要留着");
        let assistant = &clean[1];
        assert_eq!(assistant.role, Role::Assistant);
        assert!(
            assistant
                .content
                .iter()
                .all(|block| block.as_tool_call().is_none())
        );
        assert!(
            assistant
                .content
                .iter()
                .any(|block| block.as_text() == Some("我先问一句"))
        );
    }

    #[test]
    fn orphan_tool_result_is_dropped_and_normal_pair_survives() {
        let paired_call = ContentBlock::ToolCall {
            id: "ok".to_owned(),
            name: "read".to_owned(),
            arguments: "{}".to_owned(),
        };
        let messages = vec![
            Message::assistant(vec![paired_call], "deepseek", "deepseek-flash"),
            Message::tool_result("ok", "结果", false),
            Message::tool_result("orphan-result", "没有主的结果", false),
            Message::assistant(vec![call("orphan-call")], "deepseek", "deepseek-flash"),
        ];
        let clean = sanitize(messages);
        assert_eq!(clean.len(), 2, "正常 call/result 保留，孤儿都不发");
        assert!(
            clean[0]
                .content
                .iter()
                .any(|block| block.as_tool_call().is_some())
        );
        assert_eq!(clean[1].tool_call_id.as_deref(), Some("ok"));
    }
}
