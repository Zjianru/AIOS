//! 持久效果表：launch 和 relay 两种效果，以及它们的重试与健康事实。

use serde::{Deserialize, Serialize};

use crate::id::{AttemptId, ReportId, TaskId};

/// 连续失败到几次算效果种类报健康错误。
pub const FAILURES_BEFORE_ERROR: u32 = 3;

/// 效果种类；数据库里存小写字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectKind {
    Launch,
    Relay,
}

impl EffectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Relay => "relay",
        }
    }

    /// 初值和上限的退避毫秒，照 DSH：launch 5s/300s，relay 2s/60s。
    pub fn retry(self) -> (i64, i64) {
        match self {
            Self::Launch => (5_000, 300_000),
            Self::Relay => (2_000, 60_000),
        }
    }
}

/// 效果行里存的 JSON 载荷；键由载荷自己给出。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum EffectPayload {
    Launch {
        task_id: TaskId,
    },
    Relay {
        task_id: TaskId,
        report_id: ReportId,
    },
    /// 停止事实的 relay：和报告同一条 Kind 投递通路，但没有报告 id。
    /// 键里带任务 id：Attempt 编号只在任务内唯一，单用编号会让两个任务的停止互相覆盖。
    Stop {
        task_id: TaskId,
        attempt_id: AttemptId,
    },
}

impl EffectPayload {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::Launch { .. } => EffectKind::Launch,
            Self::Relay { .. } | Self::Stop { .. } => EffectKind::Relay,
        }
    }

    /// launch 的键是任务 id，relay 的键是报告 id，停止的键是任务 id + Attempt 编号。
    pub fn key(&self) -> String {
        match self {
            Self::Launch { task_id } => task_id.to_string(),
            Self::Relay { report_id, .. } => report_id.to_string(),
            Self::Stop {
                task_id,
                attempt_id,
            } => format!("stop:{task_id}:{attempt_id}"),
        }
    }
}

/// 一次领到的效果行；完成、失败或推迟时用它核对 seq 和 generation。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectClaim {
    pub payload: EffectPayload,
    pub key: String,
    pub seq: i64,
    pub generation: i64,
    pub attempts: u32,
    pub due: i64,
    pub since: i64,
}

/// 效果种类的健康状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectState {
    Ready,
    Error,
}

/// 一个效果种类的健康事实，从 effect 表现算，不存内存。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectHealth {
    pub state: EffectState,
    pub waiting: u32,
    /// 等待行里最大的连续失败次数。
    pub attempts: u32,
    pub since: Option<i64>,
    pub last_error: Option<String>,
}
