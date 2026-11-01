//! agent 收件箱：follow-up 进下一轮，steer/inject 进下一步。
//!
//! T006.2a：每次变更都写成一条 `agent/inbox/spliced` 事件（DSH agent-loop/src/inbox.ts:20-64、
//! :96-122、:158-231），进程重启时从日志折叠恢复；claimed/discarded/inserted 只是 dispatch
//! 通知、不落日志（DSH README.zh.md:71）。

use std::collections::HashSet;

use model::Message;
use session::{Event, EventKind, InboxOutcome, InboxTarget};

use crate::AgentError;

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
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Inbox {
    next_turn: Vec<Message>,
    next_step: Vec<Message>,
}

/// 一次规范化变更：对应一条 `agent/inbox/spliced` 事件，同时用来更新内存队列。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Splice {
    target: InboxTarget,
    start: usize,
    removed: Vec<Message>,
    inserted: Vec<Message>,
    /// 普通删除（取消）才带 outcome canceled；领取不带（DSH inbox.ts:214-221）。
    discard_removed: bool,
}

impl Splice {
    /// 空变更不落日志（DSH mutate: 删 0 插 0 直接返回，inbox.ts:205）。
    pub(crate) fn is_empty(&self) -> bool {
        self.removed.is_empty() && self.inserted.is_empty()
    }

    /// 被移除的消息（领取/取消的消费方从这里拿）。
    pub(crate) fn removed(&self) -> &[Message] {
        &self.removed
    }

    /// 插入的消息（inserted 通知用）。
    pub(crate) fn inserted(&self) -> &[Message] {
        &self.inserted
    }

    pub(crate) fn discard_removed(&self) -> bool {
        self.discard_removed
    }

    /// 落日志用的事件：removedCount 只在删了东西时出现，outcome 只在普通删除时出现。
    pub(crate) fn event_kind(&self) -> EventKind {
        EventKind::InboxSpliced {
            target: self.target,
            start: self.start as u64,
            removed_count: if self.removed.is_empty() {
                None
            } else {
                Some(self.removed.len() as u64)
            },
            inserted: self.inserted.clone(),
            outcome: if self.discard_removed && !self.removed.is_empty() {
                Some(InboxOutcome::Canceled)
            } else {
                None
            },
        }
    }
}

impl Inbox {
    pub(crate) fn has_pending(&self) -> bool {
        !self.next_turn.is_empty() || !self.next_step.is_empty()
    }

    pub(crate) fn next_step_is_empty(&self) -> bool {
        self.next_step.is_empty()
    }

    /// 从日志恢复：按顺序折叠 `agent/inbox/spliced`；坐标越界算日志损坏，不静默丢。
    pub(crate) fn restore(events: &[Event]) -> Result<Self, AgentError> {
        let mut inbox = Self::default();
        for event in events {
            let EventKind::InboxSpliced {
                target,
                start,
                removed_count,
                inserted,
                outcome,
            } = &event.kind
            else {
                continue;
            };
            inbox.apply_persisted(
                event.seq,
                *target,
                *start,
                *removed_count,
                inserted,
                *outcome,
            )?;
        }
        Ok(inbox)
    }

    /// 折叠一条已提交的 splice（DSH 的 inboxProjectionDefinition.apply，inbox.ts:30-55）。
    fn apply_persisted(
        &mut self,
        seq: u64,
        target: InboxTarget,
        start: u64,
        removed_count: Option<u64>,
        inserted: &[Message],
        outcome: Option<InboxOutcome>,
    ) -> Result<(), AgentError> {
        let len = self.list(target).len() as u64;
        let removed_count = removed_count.unwrap_or(0);
        // 坐标要落在 [0, len] 内。start + removedCount 必须 checked_add：removedCount 来自
        // 日志里的 u64，直接相加会溢出 panic（debug）或绕出非法 slice range（release）；
        // 溢出按损坏日志返回 CorruptInbox。
        let Some(end) = start.checked_add(removed_count).filter(|end| *end <= len) else {
            return Err(AgentError::CorruptInbox(format!(
                "invalid persisted inbox splice at session seq {seq}: target={}, start={start}, removedCount={removed_count}, len={len}",
                target.as_str()
            )));
        };
        let splice = Splice {
            target,
            start: start as usize,
            removed: self.list(target)[start as usize..end as usize].to_vec(),
            inserted: inserted.to_vec(),
            discard_removed: outcome == Some(InboxOutcome::Canceled),
        };
        // 照 DSH inbox.ts:41-48：应用后两个 pending 队列合起来的 MessageId 必须唯一。
        // 伪造/损坏的 spliced 把同一条消息插两遍，会让 resume/claim 重复投递；
        // 按损坏日志拒绝（CorruptInbox，带 seq/target/message id），不静默收下。
        if let Some(duplicate) = self.duplicate_message_id(&splice) {
            return Err(AgentError::CorruptInbox(format!(
                "duplicate message id {duplicate} in persisted inbox splice at session seq {seq}: target={}, start={start}, removedCount={removed_count}",
                target.as_str()
            )));
        }
        self.apply(&splice);
        Ok(())
    }

    /// 校验一条待应用的 splice：应用后两个 pending 队列的 MessageId 必须唯一
    /// （DSH inbox.ts:41-48）。live 路径在落盘前调用，损坏的 splice 不写日志、不改内存。
    pub(crate) fn validate(&self, splice: &Splice) -> Result<(), AgentError> {
        match self.duplicate_message_id(splice) {
            Some(duplicate) => Err(AgentError::CorruptInbox(format!(
                "duplicate message id {duplicate} in inbox splice: target={}, start={}",
                splice.target.as_str(),
                splice.start
            ))),
            None => Ok(()),
        }
    }

    /// 应用一条 splice 后第一条重复的 MessageId（没有返回 None）。
    ///
    /// 照 DSH 的折叠校验：把「目标队列应用 splice 后的视图」和「另一个队列」合起来查重，
    /// 所以 inserted 内重复、与目标队列已有消息重复、跨队列重复都能抓到。
    fn duplicate_message_id(&self, splice: &Splice) -> Option<String> {
        let list = self.list(splice.target);
        let before = list.get(..splice.start)?;
        let after = list.get(splice.start + splice.removed.len()..)?;
        let other = match splice.target {
            InboxTarget::NextTurn => &self.next_step,
            InboxTarget::NextStep => &self.next_turn,
        };
        let mut ids = HashSet::new();
        for message in before
            .iter()
            .chain(splice.inserted.iter())
            .chain(after)
            .chain(other)
        {
            if !ids.insert(message.id.as_str()) {
                return Some(message.id.as_str().to_owned());
            }
        }
        None
    }

    /// 计算一次规范化 splice（不应用）；坐标语义照 DSH mutate（inbox.ts:195-221）：
    /// 负 start 从尾部算，超出长度钳到边界，删除数量钳到可删范围。
    pub(crate) fn splice(
        &self,
        target: InboxTarget,
        start: i64,
        delete_count: i64,
        inserted: Vec<Message>,
        discard_removed: bool,
    ) -> Splice {
        let list = self.list(target);
        let len = list.len() as i64;
        let resolved_start = if start < 0 {
            (len + start).max(0)
        } else {
            start.min(len)
        };
        let removed_count = delete_count.clamp(0, len - resolved_start) as usize;
        let start = resolved_start as usize;
        Splice {
            target,
            start,
            removed: list[start..start + removed_count].to_vec(),
            inserted,
            discard_removed,
        }
    }

    /// 应用一条已经落盘的 splice。
    pub(crate) fn apply(&mut self, splice: &Splice) {
        let start = splice.start;
        self.list_mut(splice.target).splice(
            start..start + splice.removed.len(),
            splice.inserted.iter().cloned(),
        );
    }

    fn list(&self, target: InboxTarget) -> &Vec<Message> {
        match target {
            InboxTarget::NextTurn => &self.next_turn,
            InboxTarget::NextStep => &self.next_step,
        }
    }

    fn list_mut(&mut self, target: InboxTarget) -> &mut Vec<Message> {
        match target {
            InboxTarget::NextTurn => &mut self.next_turn,
            InboxTarget::NextStep => &mut self.next_step,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn splice_event(seq: u64, target: InboxTarget, start: u64, inserted: Vec<Message>) -> Event {
        Event::new(
            seq,
            seq,
            EventKind::InboxSpliced {
                target,
                start,
                removed_count: None,
                inserted,
                outcome: None,
            },
        )
    }

    #[test]
    fn restore_rejects_duplicate_message_id() {
        // 照 DSH inbox.ts:41-48：折叠后两个 pending 队列的 MessageId 必须唯一；
        // 伪造/损坏的 spliced 把同一条消息插两遍 → CorruptInbox，不静默收下。
        let pending = Message::user_text("排队中");
        let events = vec![
            splice_event(1, InboxTarget::NextTurn, 0, vec![pending.clone()]),
            splice_event(2, InboxTarget::NextTurn, 1, vec![pending.clone()]),
        ];
        let error = Inbox::restore(&events).expect_err("重复 MessageId 是损坏日志");
        assert!(
            matches!(&error, AgentError::CorruptInbox(detail)
                if detail.contains(pending.id.as_str()) && detail.contains("seq 2")),
            "要报 CorruptInbox 且带 seq/message id：{error}"
        );
    }

    #[test]
    fn live_splice_rejects_duplicate_message_id() {
        // live 路径：send/context 每次都生成新 uuid，重复 MessageId 只能来自损坏的调用方；
        // 落盘前的共用校验必须拒绝（不写坏日志、不改内存）。
        let pending = Message::user_text("在队列里");
        let mut inbox = Inbox::default();
        let seed = inbox.splice(InboxTarget::NextTurn, 0, 0, vec![pending.clone()], false);
        inbox.apply(&seed);

        // 同一队列再插一次同 id。
        let duplicate = inbox.splice(
            InboxTarget::NextTurn,
            i64::MAX,
            0,
            vec![pending.clone()],
            false,
        );
        let error = inbox
            .validate(&duplicate)
            .expect_err("live splice 的重复 MessageId 要拒绝");
        assert!(
            matches!(&error, AgentError::CorruptInbox(detail)
                if detail.contains(pending.id.as_str())),
            "要报 CorruptInbox 且带 message id：{error}"
        );

        // 跨队列（next-turn 已有，next-step 再插同 id）也拒绝：DSH 查两个队列的并集。
        let across = inbox.splice(
            InboxTarget::NextStep,
            i64::MAX,
            0,
            vec![pending.clone()],
            false,
        );
        let error = inbox
            .validate(&across)
            .expect_err("跨队列重复 MessageId 要拒绝");
        assert!(
            matches!(&error, AgentError::CorruptInbox(detail)
                if detail.contains(pending.id.as_str())),
            "要报 CorruptInbox 且带 message id：{error}"
        );
    }
}
