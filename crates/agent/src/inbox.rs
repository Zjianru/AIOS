//! agent 收件箱：follow-up 进下一轮，steer/inject 进下一步。

use model::{Message, UserMessage};
use session::InboxTarget;

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
    /// 这条投递会不会唤醒空闲的 loop。
    pub(crate) fn wakes(self) -> bool {
        !matches!(self, Self::Inject)
    }

    pub(crate) fn target(self) -> InboxTarget {
        match self {
            Self::FollowUp => InboxTarget::NextTurn,
            Self::Steer | Self::Inject => InboxTarget::NextStep,
        }
    }
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

    /// 领取一个 step 的输入：全部 next-step，外加（请求时）一条 next-turn。
    pub(crate) fn claim(&mut self, target: InboxTarget) -> Vec<Message> {
        let mut claimed = std::mem::take(&mut self.next_step);
        if target == InboxTarget::NextTurn && !self.next_turn.is_empty() {
            claimed.push(self.next_turn.remove(0));
        }
        claimed
    }

    pub(crate) fn has_pending(&self) -> bool {
        !self.next_turn.is_empty() || !self.next_step.is_empty()
    }

    pub(crate) fn next_step_is_empty(&self) -> bool {
        self.next_step.is_empty()
    }

    pub(crate) fn clear(&mut self) {
        self.next_turn.clear();
        self.next_step.clear();
    }
}
