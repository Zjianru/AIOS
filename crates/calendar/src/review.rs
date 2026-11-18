//! 复核（纯逻辑）：按固定顺序判跳过/排着/送；不读时钟、不碰库。

use crate::types::{Missed, Review, ReviewFacts};

/// 复核顺序照 spec：暂停 → 绑定任务 → 上一次还没完 → 全局暂停 → 盯变化 → 送。
pub fn review(facts: &ReviewFacts) -> Review {
    use crate::types::{AppointmentStatus, Review, WatchOutcome};
    if facts.status == AppointmentStatus::Paused {
        return Review::Skip("跳过：约定已暂停".to_owned());
    }
    if matches!(
        facts.status,
        AppointmentStatus::Cancelled | AppointmentStatus::Ended
    ) {
        return Review::NoRun;
    }
    if let Some(task) = &facts.bound_task {
        if !task.exists || task.graph_cancelled {
            return Review::Skip("跳过：绑定的任务已不在".to_owned());
        }
        if task.closed {
            return Review::End;
        }
    }
    if facts.kind_busy {
        return Review::Skip("跳过：上一次 Kind 还在处理".to_owned());
    }
    if facts.bound_task.as_ref().is_some_and(|task| task.running) {
        return Review::Skip("跳过：任务这一轮还在跑".to_owned());
    }
    if facts.global_paused {
        return Review::Queue("排着：全局暂停中".to_owned());
    }
    match &facts.watch {
        Some(WatchOutcome::Failed(reason)) => Review::Fail(format!("没送成：{reason}")),
        Some(WatchOutcome::Unchanged) => Review::Skip("没变：和上次比内容一样".to_owned()),
        Some(WatchOutcome::Changed) | None => Review::Send,
    }
}

/// 全局暂停期间到点的多次合并成一次送。
pub fn merged_reason_global_pause(count: u32) -> String {
    format!("合并了 {count} 次：全局暂停期间到点")
}

/// 服务没在跑（电脑睡着/进程停过）期间错过的多次合并成一次送。
pub fn merged_reason_missed(count: u32) -> String {
    format!("合并了 {count} 次：服务没在跑期间到点")
}

/// 错过的触发点只补最近一次，并记合并了几次（len-1）。
pub fn merge_missed(occurrences: &[i64]) -> Missed {
    match occurrences {
        [] => Missed::default(),
        _ => Missed {
            latest: occurrences.last().copied(),
            merged: u32::try_from(occurrences.len() - 1).unwrap_or(u32::MAX),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{AppointmentStatus, ReviewFacts, WatchOutcome};

    fn facts(status: AppointmentStatus) -> ReviewFacts {
        ReviewFacts {
            status,
            bound_task: None,
            kind_busy: false,
            global_paused: false,
            watch: None,
        }
    }

    #[test]
    fn paused_skips_with_fixed_words() {
        let mut facts = facts(AppointmentStatus::Paused);
        facts.bound_task = None;
        assert_eq!(review(&facts), Review::Skip("跳过：约定已暂停".to_owned()));
    }

    #[test]
    fn cancelled_and_ended_never_start_a_run() {
        assert_eq!(review(&facts(AppointmentStatus::Cancelled)), Review::NoRun);
        assert_eq!(review(&facts(AppointmentStatus::Ended)), Review::NoRun);
    }

    #[test]
    fn missing_or_cancelled_bound_task_skips() {
        let mut facts = facts(AppointmentStatus::Active);
        facts.bound_task = Some(crate::types::BoundTaskFacts {
            exists: false,
            graph_cancelled: false,
            closed: false,
            running: false,
        });
        assert_eq!(
            review(&facts),
            Review::Skip("跳过：绑定的任务已不在".to_owned())
        );
        facts.bound_task = Some(crate::types::BoundTaskFacts {
            exists: true,
            graph_cancelled: true,
            closed: false,
            running: false,
        });
        assert_eq!(
            review(&facts),
            Review::Skip("跳过：绑定的任务已不在".to_owned())
        );
    }

    #[test]
    fn closed_bound_task_ends_the_appointment() {
        let mut facts = facts(AppointmentStatus::Active);
        facts.bound_task = Some(crate::types::BoundTaskFacts {
            exists: true,
            graph_cancelled: false,
            closed: true,
            running: false,
        });
        assert_eq!(review(&facts), Review::End);
    }

    #[test]
    fn previous_turn_still_running_skips_with_fixed_words() {
        let mut facts = facts(AppointmentStatus::Active);
        facts.kind_busy = true;
        assert_eq!(
            review(&facts),
            Review::Skip("跳过：上一次 Kind 还在处理".to_owned())
        );
        facts.kind_busy = false;
        facts.bound_task = Some(crate::types::BoundTaskFacts {
            exists: true,
            graph_cancelled: false,
            closed: false,
            running: true,
        });
        assert_eq!(
            review(&facts),
            Review::Skip("跳过：任务这一轮还在跑".to_owned())
        );
    }

    #[test]
    fn global_pause_queues_then_merges() {
        let mut facts = facts(AppointmentStatus::Active);
        facts.global_paused = true;
        assert_eq!(review(&facts), Review::Queue("排着：全局暂停中".to_owned()));
        assert_eq!(
            merged_reason_global_pause(3),
            "合并了 3 次：全局暂停期间到点"
        );
        assert_eq!(merged_reason_missed(2), "合并了 2 次：服务没在跑期间到点");
    }

    #[test]
    fn watch_failure_and_unchanged_do_not_send() {
        let mut facts = facts(AppointmentStatus::Active);
        facts.watch = Some(WatchOutcome::Failed("取不到网页（超时）".to_owned()));
        assert_eq!(
            review(&facts),
            Review::Fail("没送成：取不到网页（超时）".to_owned())
        );
        facts.watch = Some(WatchOutcome::Unchanged);
        assert_eq!(
            review(&facts),
            Review::Skip("没变：和上次比内容一样".to_owned())
        );
        facts.watch = Some(WatchOutcome::Changed);
        assert_eq!(review(&facts), Review::Send);
    }

    #[test]
    fn plain_due_review_sends() {
        assert_eq!(review(&facts(AppointmentStatus::Active)), Review::Send);
    }

    #[test]
    fn merged_missed_only_keeps_the_latest() {
        let merged = merge_missed(&[100, 200, 300]);
        assert_eq!(merged.latest, Some(300));
        assert_eq!(merged.merged, 2);
        assert_eq!(merge_missed(&[]), crate::types::Missed::default());
        assert_eq!(
            merge_missed(&[42]),
            crate::types::Missed {
                latest: Some(42),
                merged: 0
            }
        );
    }
}
