//! 会话事件与 header。

use model::{Message, Usage};
use serde::{Deserialize, Serialize};

use crate::SessionId;

/// 会话日志的格式版本；读到不认识的版本就拒绝。
pub const FORMAT_VERSION: u64 = 1;

/// JSONL 第一行的会话 header。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionHeader {
    #[serde(rename = "type")]
    kind: String,
    pub version: u64,
    pub id: SessionId,
    #[serde(rename = "createdAt")]
    pub created_at: u64,
    pub cwd: String,
}

impl SessionHeader {
    pub(crate) fn new(id: SessionId, cwd: String, created_at: u64) -> Self {
        Self {
            kind: "session".to_owned(),
            version: FORMAT_VERSION,
            id,
            created_at,
            cwd,
        }
    }

    /// 校验 header 的 type 标记。
    pub(crate) fn validate(&self) -> Result<(), crate::SessionError> {
        if self.kind == "session" {
            Ok(())
        } else {
            Err(crate::SessionError::CorruptHeader(format!(
                "第一行的 type 是 {}，不是 session",
                self.kind
            )))
        }
    }
}

/// 一轮结束的原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TurnEndReason {
    Completed,
    MaxTokens,
    Aborted,
    Error { code: String, message: String },
}

/// todo 条目的状态（wire 名照 DSH：pending / in_progress / completed）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
}

impl TodoStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::InProgress => "in_progress",
            Self::Completed => "completed",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "pending" => Some(Self::Pending),
            "in_progress" => Some(Self::InProgress),
            "completed" => Some(Self::Completed),
            _ => None,
        }
    }
}

/// 一个 todo 条目（整表替换的最小形状，照 DSH todo_write：content + 三态 status）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TodoItem {
    pub content: String,
    pub status: TodoStatus,
}

/// 会话事件。类型集合照 spec「实现决定·会话日志」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum EventKind {
    #[serde(rename = "turn/start")]
    TurnStart { turn: u64 },
    #[serde(rename = "turn/end")]
    TurnEnd { turn: u64, reason: TurnEndReason },
    #[serde(rename = "step/start")]
    StepStart { turn: u64, step: u64 },
    #[serde(rename = "step/end")]
    StepEnd { turn: u64, step: u64 },
    #[serde(rename = "system/message")]
    SystemMessage {
        turn: u64,
        step: u64,
        message: Message,
    },
    #[serde(rename = "user/message")]
    UserMessage {
        turn: u64,
        step: u64,
        message: Message,
    },
    #[serde(rename = "assistant/message")]
    AssistantMessage {
        turn: u64,
        step: u64,
        message: Message,
        #[serde(skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        #[serde(skip_serializing_if = "Option::is_none")]
        interrupted: Option<bool>,
    },
    /// 模型请求的一次工具调用（持久化事实；assistant/message 里也有同一调用块）。
    #[serde(rename = "tool/call")]
    ToolCall {
        turn: u64,
        step: u64,
        call_id: String,
        name: String,
        arguments: String,
    },
    /// 一次工具调用的结果，按模型给出的顺序提交。
    #[serde(rename = "tool/result")]
    ToolResult {
        turn: u64,
        step: u64,
        message: Message,
        #[serde(skip_serializing_if = "Option::is_none")]
        error_code: Option<String>,
    },
    /// 一个旧工具结果被修剪：派生请求里同 call_id 的 tool/result 用 content 替换。
    /// 原始 tool/result 事件保留在日志里（可回放），这条只记替换视图（T005.2）。
    #[serde(rename = "compaction/prune")]
    CompactionPrune { call_id: String, content: String },
    /// 模型提交的整份 todo 列表快照；回放时后写覆盖先写（T005.7，类型名照 DSH todo/write）。
    #[serde(rename = "todo/write")]
    TodoWrite {
        turn: u64,
        step: u64,
        todos: Vec<TodoItem>,
    },
    /// 一次压缩开始（T005.8a；类型名照 DSH known-event-types）。锁语义简化：AIOS 单写者，
    /// start 先落、end 收尾，失败时 end 带 error。
    #[serde(rename = "compaction/start")]
    CompactionStart { compaction_id: String, turn: u64 },
    /// 一次压缩完成：摘要文本 + 被替换事件的 seq 区间 + 被替换内容的估算 token 数。
    /// 原始事件保留在日志里；派生请求用一条 checkpoint 消息替换该区间。
    #[serde(rename = "compaction/summary")]
    CompactionSummary {
        compaction_id: String,
        summary: String,
        shadowed_start: u64,
        shadowed_end: u64,
        shadowed_token_count: u64,
    },
    /// 一次压缩结束；error 为 Some 表示失败（日志仍只追加）。
    #[serde(rename = "compaction/end")]
    CompactionEnd {
        compaction_id: String,
        turn: u64,
        error: Option<String>,
    },
}

impl EventKind {
    /// 事件的 wire 类型名（测试与日志诊断用）。
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::TurnStart { .. } => "turn/start",
            Self::TurnEnd { .. } => "turn/end",
            Self::StepStart { .. } => "step/start",
            Self::StepEnd { .. } => "step/end",
            Self::SystemMessage { .. } => "system/message",
            Self::UserMessage { .. } => "user/message",
            Self::AssistantMessage { .. } => "assistant/message",
            Self::ToolCall { .. } => "tool/call",
            Self::ToolResult { .. } => "tool/result",
            Self::CompactionPrune { .. } => "compaction/prune",
            Self::TodoWrite { .. } => "todo/write",
            Self::CompactionStart { .. } => "compaction/start",
            Self::CompactionSummary { .. } => "compaction/summary",
            Self::CompactionEnd { .. } => "compaction/end",
        }
    }
}

/// 从日志回放当前 todo 列表：最近一次 todo/write 胜出（T005.7 续接恢复用）。
pub fn last_todos(events: &[Event]) -> Option<Vec<TodoItem>> {
    events.iter().rev().find_map(|event| match &event.kind {
        EventKind::TodoWrite { todos, .. } => Some(todos.clone()),
        _ => None,
    })
}

/// 日志里的一条事件：seq 从 1 开始连续，time 是 unix 毫秒。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Event {
    pub seq: u64,
    pub time: u64,
    #[serde(flatten)]
    pub kind: EventKind,
}
