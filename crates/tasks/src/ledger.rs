//! T056.2：账本的叙述条（否掉的做法 / 下一步）。进展、待答从报告现编，不存这里；
//! 约束跟着要求版本走，也不进这张表。

use serde::{Deserialize, Serialize};

use crate::{AttemptId, TaskId};

/// 叙述条只有两类。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LedgerNoteKind {
    /// 否掉的做法（试过、不再走）。
    Rejected,
    /// 下一步（要停、要交回时留给下一次）。
    Next,
}

impl LedgerNoteKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Rejected => "rejected",
            Self::Next => "next",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "rejected" => Some(Self::Rejected),
            "next" => Some(Self::Next),
            _ => None,
        }
    }
}

/// ThinAgent 要记的一条叙述；topic 相同的旧条会被新条替换。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewLedgerNote {
    pub kind: LedgerNoteKind,
    pub topic: String,
    pub text: String,
}

/// 一行叙述条（开着或已关闭）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LedgerNote {
    pub id: String,
    pub task_id: TaskId,
    pub attempt_id: AttemptId,
    pub kind: LedgerNoteKind,
    pub topic: String,
    pub text: String,
    pub at: i64,
    pub closed_at: Option<i64>,
    pub closed_why: Option<String>,
}

/// 记完以后的在用条数（工具回执用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LedgerNoteCounts {
    pub rejected: u64,
    pub next: u64,
}
