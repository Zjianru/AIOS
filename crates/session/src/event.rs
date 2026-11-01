//! 会话事件与 header。

use model::{Message, Usage};
use serde::{Deserialize, Serialize};

use crate::SessionId;

/// 会话日志的格式版本；读到不认识的版本就拒绝，不做迁移。
///
/// v2：事件记录照 DSH 的 SessionEvent：`{type, seq, time, data}`，事件自己的字段放进
/// data（DSH `packages/core/session/src/index.ts:826-836`、
/// `packages/session/session-format-v1-to-v2/src/codec.ts:21`）。v1 的日志直接拒绝，
/// 不做迁移（spec 实现决定：现在没有产品数据）。
pub const FORMAT_VERSION: u64 = 2;

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

/// 一轮被取消的原因（DSH 的 TurnEndCancelCause 子集；AIOS 目前只有用户/调用方取消）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AbortCause {
    User,
}

/// turn/end 的 error 原因（DSH 的 LlmFailure 子集：AIOS 只记 message 和 code）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmFailure {
    pub message: String,
    pub code: String,
}

/// 一轮结束的原因（DSH TurnEndReasonMap 的子集：只列 AIOS 会产生的几类）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum TurnEndReason {
    Completed,
    MaxTokens,
    /// 调用方/用户取消。
    Aborted {
        reason: AbortCause,
    },
    /// 崩溃/被杀后由续接修复补的收尾（照 DSH repair.ts:96）。
    Interrupted,
    /// 一轮以错误收尾；error 是 DSH LlmFailure 的子集。
    Error {
        error: LlmFailure,
    },
}

/// 一次工具失败的身份（DSH 的 tool/result.error：name + code）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolError {
    pub name: String,
    pub code: String,
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
    /// 模型向用户提问（T005.10；派生请求不出现，只用于日志回放/CLI 展示）。
    #[serde(rename = "question/asked")]
    QuestionAsked {
        turn: u64,
        step: u64,
        question_id: String,
        question: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        header: Option<String>,
        options: Vec<String>,
        multi_select: bool,
    },
    /// 等待中被取消，问题撤销（T005.10）。
    #[serde(rename = "question/cancelled")]
    QuestionCancelled {
        turn: u64,
        step: u64,
        question_id: String,
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
        #[serde(rename = "callId")]
        call_id: String,
        name: String,
        arguments: String,
    },
    /// 一次工具调用的结果，按模型给出的顺序提交。error 照 DSH：失败身份在模型可见内容之外。
    #[serde(rename = "tool/result")]
    ToolResult {
        turn: u64,
        step: u64,
        message: Message,
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<ToolError>,
    },
    /// 一个旧工具结果被修剪：派生请求里同 call_id 的 tool/result 用 content 替换。
    /// 原始 tool/result 事件保留在日志里（可回放），这条只记替换视图（T005.2）。
    #[serde(rename = "compaction/prune")]
    CompactionPrune { call_id: String, content: String },
    /// 模型提交的整份 todo 列表快照；回放时后写覆盖先写（T005.7，类型名照 DSH todo/write）。
    #[serde(rename = "todo/write")]
    TodoWrite { todos: Vec<TodoItem> },
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
            Self::QuestionAsked { .. } => "question/asked",
            Self::QuestionCancelled { .. } => "question/cancelled",
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
        EventKind::TodoWrite { todos } => Some(todos.clone()),
        _ => None,
    })
}

/// 日志里的一条事件：`{type, seq, time, data}`（照 DSH 的 SessionEvent）。
///
/// seq 从 1 开始连续，time 是 unix 毫秒；事件自己的字段放在 data 里。
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    pub seq: u64,
    pub time: u64,
    pub kind: EventKind,
}

impl Serialize for Event {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::{Error as _, SerializeMap as _};
        let serde_json::Value::Object(mut data) =
            serde_json::to_value(&self.kind).map_err(S::Error::custom)?
        else {
            return Err(S::Error::custom("事件字段不是 JSON 对象"));
        };
        let kind = data
            .remove("type")
            .ok_or_else(|| S::Error::custom("事件字段缺少 type"))?;
        let mut map = serializer.serialize_map(Some(4))?;
        map.serialize_entry("type", &kind)?;
        map.serialize_entry("seq", &self.seq)?;
        map.serialize_entry("time", &self.time)?;
        map.serialize_entry("data", &serde_json::Value::Object(data))?;
        map.end()
    }
}

impl<'de> Deserialize<'de> for Event {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        #[derive(Deserialize)]
        struct WireEvent {
            #[serde(rename = "type")]
            kind: String,
            seq: u64,
            time: u64,
            data: serde_json::Value,
        }
        let wire = WireEvent::deserialize(deserializer)?;
        let serde_json::Value::Object(mut data) = wire.data else {
            return Err(D::Error::custom("事件的 data 必须是 JSON 对象"));
        };
        data.insert("type".to_owned(), serde_json::Value::String(wire.kind));
        let kind =
            serde_json::from_value(serde_json::Value::Object(data)).map_err(D::Error::custom)?;
        Ok(Self {
            seq: wire.seq,
            time: wire.time,
            kind,
        })
    }
}
