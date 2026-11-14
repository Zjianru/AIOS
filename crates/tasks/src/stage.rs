//! 阶段派生：按 docs/03 5.1 的顺序取第一个命中的，只覆盖当前能派生出来的值。

use crate::task::{AttemptEnd, ReportState, Task};

/// docs/03 5.1 的六个阶段；T054 起补上「已关闭」。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 任务包缺项，最近报告 blocked/failed，或最近 Attempt 以 stopped/interrupted 结束。
    NeedsConditions,
    /// 没有 Attempt，或 launch 还在排队。
    Pending,
    /// 有 Attempt，包括合成的 waiting。
    Running,
    /// 最近报告 ready。
    ReadyForReview,
    /// T053：门对最近报告判了，或门判了通过的候选就是被把关任务的最近 ready 报告。
    RequirementMet,
    /// T054：当前版本的最新报告被用户接受（有门的还要门判通过）；子任务全关闭的总任务也到这。
    Closed,
}

impl Stage {
    /// 表序（docs/03 5.1）：数字越小越靠前；总任务汇总取最靠前的子任务阶段。
    pub(crate) fn rank(self) -> u8 {
        match self {
            Self::NeedsConditions => 0,
            Self::Pending => 1,
            Self::Running => 2,
            Self::ReadyForReview => 3,
            Self::RequirementMet => 4,
            Self::Closed => 5,
        }
    }
}

impl Task {
    /// 按 spec 的表序派生阶段。launch 是否排队由调用方从效果表查出来传进来。
    pub fn stage(&self, launch_queued: bool) -> Stage {
        if !self.missing.is_empty() {
            return Stage::NeedsConditions;
        }
        let latest_report = self.latest_report().map(|report| report.state);
        match latest_report {
            Some(ReportState::Blocked) => return Stage::NeedsConditions,
            Some(ReportState::Failed) => return Stage::NeedsConditions,
            Some(ReportState::Waiting) | None => {}
            // 待检查排在已停止后面：停止一旦落下，结果就定死，不被旧报告盖过。
            Some(ReportState::Ready) => {}
        }
        if self
            .latest_attempt()
            .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Stopped))
        {
            return Stage::NeedsConditions;
        }
        if self
            .latest_attempt()
            .is_some_and(|attempt| attempt.end == Some(AttemptEnd::Interrupted))
        {
            return Stage::NeedsConditions;
        }
        if latest_report == Some(ReportState::Ready) {
            return Stage::ReadyForReview;
        }
        if !self.has_attempt() || launch_queued {
            return Stage::Pending;
        }
        Stage::Running
    }
}
