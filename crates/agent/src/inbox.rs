//! agent 收件箱：follow-up 进下一轮，steer/inject 进下一步。
//!
//! T006.2a：每次变更都写成一条 `agent/inbox/spliced` 事件（DSH agent-loop/src/inbox.ts:20-64、
//! :96-122、:158-231），进程重启时从日志折叠恢复；claimed/discarded/inserted 只是 dispatch
//! 通知、不落日志（DSH README.zh.md:71）。

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
    ///
    /// AIOS 的 Message 还没有稳定 id（T006.5c1 才有），这里只校验坐标边界；
    /// DSH 的重复 MessageId 校验随 5c1 补（handback known）。
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
        // 只有 canceled 一种 outcome；折叠时不影响队列状态。
        let _ = outcome;
        self.list_mut(target)
            .splice(start as usize..end as usize, inserted.iter().cloned());
        Ok(())
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
