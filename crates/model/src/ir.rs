//! provider 无关的消息、用量和流事件。

use serde::{Deserialize, Serialize};

/// 一条消息的角色。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

/// 消息内容块。T004.1 只有文本；推理块和工具块在后续票加入。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text { text: String },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// 文本块的内容；非文本块返回 `None`。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
        }
    }
}

/// 一条 provider 无关的消息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: Vec<ContentBlock>,
}

impl Message {
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self { role, content }
    }

    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self {
            role,
            content: vec![ContentBlock::text(text)],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

/// 用户消息：角色固定在 `Role::User` 的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessage(Message);

impl UserMessage {
    pub fn text(text: impl Into<String>) -> Self {
        Self(Message::text(Role::User, text))
    }

    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self(Message::new(Role::User, content))
    }

    pub fn as_message(&self) -> &Message {
        &self.0
    }

    pub fn into_message(self) -> Message {
        self.0
    }
}

/// 一次请求的用量；厂商没给的项保持 `None`（显示为「未知」，不是 0）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cache_creation_input_tokens: Option<u64>,
}

impl Usage {
    /// 一个字段都没拿到（厂商什么都没给）。
    pub fn is_empty(&self) -> bool {
        self.input_tokens.is_none()
            && self.output_tokens.is_none()
            && self.cache_read_input_tokens.is_none()
            && self.cache_creation_input_tokens.is_none()
    }
}

/// 推理强度。默认 `High`（spec 实现决定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    Off,
    Low,
    #[default]
    High,
    Max,
}

impl Effort {
    /// wire 上的取值（`off`、`low`、`high`、`max`）。
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    /// 解析 CLI 传入的取值；非法值返回 `None`。
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "off" => Some(Self::Off),
            "low" => Some(Self::Low),
            "high" => Some(Self::High),
            "max" => Some(Self::Max),
            _ => None,
        }
    }
}

/// 流的结束原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FinishReason {
    /// 模型正常说完（`end_turn` / `stop_sequence`）。
    Stop,
    /// 模型要调工具。
    ToolCalls,
    /// 撞上输出上限。
    MaxTokens,
}

/// 内容块类型（流事件里只带类型，不带内容）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    Text,
}

/// provider 无关的流事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamChunk {
    BlockStart { index: usize, block_type: BlockType },
    TextDelta { index: usize, text: String },
    BlockEnd { index: usize, block: ContentBlock },
    Usage(Usage),
    Finish(FinishReason),
}
