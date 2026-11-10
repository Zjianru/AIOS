//! 阶段派生：按 docs/03 5.1 的顺序取第一个命中的，只覆盖 T024.1 可达的值。

use crate::task::{ReportState, Task};

/// docs/03 5.1 的六个阶段里，本票能派生出来的四个。
///
/// 「任务要求已满足」和「已关闭」归质量门和停止票，本票不返回。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// 任务包缺项，或最近报告 blocked/failed。
    NeedsConditions,
    /// 没有 Attempt，或 launch 还在排队。
    Pending,
    /// 有 Attempt，包括合成的 waiting。
    Running,
    /// 最近报告 ready。
    ReadyForReview,
}

impl Task {
    /// 按 spec 的表序派生阶段。launch 是否排队由调用方从效果表查出来传进来。
    pub fn stage(&self, launch_queued: bool) -> Stage {
        if !self.missing.is_empty() {
            return Stage::NeedsConditions;
        }
        match self.latest_report().map(|report| report.state) {
            Some(ReportState::Blocked) => return Stage::NeedsConditions,
            Some(ReportState::Failed) => return Stage::NeedsConditions,
            Some(ReportState::Ready) => return Stage::ReadyForReview,
            Some(ReportState::Waiting) | None => {}
        }
        if !self.has_attempt() || launch_queued {
            return Stage::Pending;
        }
        Stage::Running
    }
}
