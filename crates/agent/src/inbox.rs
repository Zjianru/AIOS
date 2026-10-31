//! agent 收件箱：follow-up 进下一轮，steer/inject 进下一步。

use model::{Message, UserMessage};

/// 消息投递方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// 下一轮：这一轮结束后自动开新一轮。
    FollowUp,
    /// 下一步：下一个 step 的请求里带上它，并唤醒空闲的 loop。
    Steer,
    /// 下一步：下一个 step 的请求里带上它，但不唤醒空闲的 loop。
    Inject,
}

impl Delivery {
    pub(crate) fn target(self) -> InboxTarget {
        match self {
            Self::FollowUp => InboxTarget::NextTurn,
            Self::Steer | Self::Inject => InboxTarget::NextStep,
        }
    }
}

/// 待领取的两条队列。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InboxTarget {
    NextTurn,
    NextStep,
}

/// 待领取的用户消息。
#[derive(Debug, Default)]
pub(crate) struct Inbox {
    next_turn: Vec<Message>,
    next_step: Vec<Message>,
}

impl Inbox {
    pub(crate) fn push(&mut self, target: InboxTarget, message: UserMessage) {
        match target {
            InboxTarget::NextTurn => self.next_turn.push(message.into_message()),
            InboxTarget::NextStep => self.next_step.push(message.into_message()),
        }
    }

    pub(crate) fn clear(&mut self) {
        self.next_turn.clear();
        self.next_step.clear();
    }
}
