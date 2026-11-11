//! 持久效果表：launch、relay、delivery、notice 和 restart，以及它们的重试与健康事实。

use serde::{Deserialize, Serialize};

use crate::id::{AttemptId, BootId, InputId, ReportId, TaskId};

/// 连续失败到几次算效果种类报健康错误。
pub const FAILURES_BEFORE_ERROR: u32 = 3;

/// 效果种类；数据库里存小写字符串。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EffectKind {
    Launch,
    Relay,
    Delivery,
    Notice,
    Restart,
}

impl EffectKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Launch => "launch",
            Self::Relay => "relay",
            Self::Delivery => "delivery",
            Self::Notice => "notice",
            Self::Restart => "restart",
        }
    }

    /// 初值和上限的退避毫秒，照 DSH：launch 5s/300s，relay 2s/60s；
    /// delivery 和 notice 是即时投递，跟 relay 同一档。
    pub fn retry(self) -> (i64, i64) {
        match self {
            Self::Launch => (5_000, 300_000),
            Self::Relay | Self::Delivery | Self::Notice | Self::Restart => (2_000, 60_000),
        }
    }

    /// 连续失败不报健康错误（DSH app/effects.ts 的 handler 选项 quiet：
    /// 失败已经在任务上体现时，不把效果种类的健康项标成 error）。
    ///
    /// delivery 照 DSH 注册为 quiet（executor/dsh.ts:234-235）；其余种类照旧。
    pub fn quiet(self) -> bool {
        matches!(self, Self::Delivery)
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
    /// 任务里有了新输入，叫投递方把还没送的那些送进任务会话。
    ///
    /// 键是任务 id：同一个任务的新输入合并成一条效果，投递方按输入 id 逐条判重。
    Delivery {
        task_id: TaskId,
    },
    /// 抄送 Kind：键里带输入 id，每条输入各送一次。
    Notice {
        task_id: TaskId,
        input_id: InputId,
    },
    /// 启动扫描认出这次启动有被打断的任务，请投递方叫 Kind 汇报并出确认卡。
    ///
    /// 键是启动 id：同一次启动认出的任务合并成一条效果，Kind 一次汇报完。
    /// 载荷里记下这次扫描打断的任务：投递时只报这些，跨启动不重复追问。
    /// 旧库里的行没有这个字段，按空读；投递方退回「全部还被打断的」，不吞掉那次汇报。
    Restart {
        boot_id: BootId,
        #[serde(default)]
        task_ids: Vec<TaskId>,
    },
}

impl EffectPayload {
    pub fn kind(&self) -> EffectKind {
        match self {
            Self::Launch { .. } => EffectKind::Launch,
            Self::Relay { .. } | Self::Stop { .. } => EffectKind::Relay,
            Self::Delivery { .. } => EffectKind::Delivery,
            Self::Notice { .. } => EffectKind::Notice,
            Self::Restart { .. } => EffectKind::Restart,
        }
    }

    /// launch 和 delivery 的键是任务 id，relay 的键是报告 id，停止的键是任务 id +
    /// Attempt 编号，抄送的键是输入 id，restart 的键是启动 id。
    pub fn key(&self) -> String {
        match self {
            Self::Launch { task_id } | Self::Delivery { task_id } => task_id.to_string(),
            Self::Relay { report_id, .. } => report_id.to_string(),
            Self::Stop {
                task_id,
                attempt_id,
            } => format!("stop:{task_id}:{attempt_id}"),
            Self::Notice { input_id, .. } => format!("cc:{input_id}"),
            Self::Restart { boot_id, .. } => boot_id.to_string(),
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
