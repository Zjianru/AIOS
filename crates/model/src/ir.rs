//! provider 无关的消息、用量和流事件。

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// 一条消息的稳定 id（DSH MessageId：uuid v4 字符串）。
///
/// 新建时生成；写进会话日志后，序列化、续接、派生都保持同一个值。
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MessageId(String);

impl MessageId {
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }
}

impl From<&str> for MessageId {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

/// 一条消息的来源（DSH MessageSourceMap 的四个 kind）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MessageSource {
    /// 用户输入或注入的上下文。
    #[default]
    User,
    /// 模型产出；provider/model 是产出该消息的路由，replayState 是 adapter 回放用的原生状态。
    Model {
        provider: String,
        model: String,
        #[serde(
            rename = "replayState",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        replay_state: Option<Value>,
    },
    /// 工具结果；callId 指向对应的工具调用。
    Tool {
        #[serde(rename = "callId")]
        call_id: String,
    },
    /// 渲染后的系统提示。
    SystemPrompt,
}

/// 一条消息的角色（五种角色照 DSH MessageRoleMap）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    Developer,
    User,
    Assistant,
    /// 工具结果消息（wire 上是 user 角色 + tool_result 块）。
    Tool,
}

/// 消息内容块。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    /// 模型的思考内容（thinking）；signature 是回放用的原生元数据（块级，照 DSH）。
    Reasoning {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
    },
    /// 模型请求的一次工具调用；arguments 是模型给出的原始 JSON 字符串。
    ToolCall {
        id: String,
        name: String,
        arguments: String,
    },
    /// 一次工具调用的结果，放在 tool/result 事件的消息里。
    ToolResult {
        tool_use_id: String,
        content: String,
        is_error: bool,
    },
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// 文本块的内容；非文本块返回 None。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Reasoning { .. } | Self::ToolCall { .. } | Self::ToolResult { .. } => None,
        }
    }

    /// 工具调用块的 (id, name, arguments)；非工具调用块返回 None。
    pub fn as_tool_call(&self) -> Option<(&str, &str, &str)> {
        match self {
            Self::ToolCall {
                id,
                name,
                arguments,
            } => Some((id, name, arguments)),
            Self::Text { .. } | Self::Reasoning { .. } | Self::ToolResult { .. } => None,
        }
    }
}

/// 一条 provider 无关的消息（DSH MessageBase + 角色字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// 稳定 id；序列化/续接/派生都保持不变。
    #[serde(skip)]
    pub id: MessageId,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// 生产者声明的来源。
    #[serde(skip)]
    pub source: MessageSource,
    /// 仅 tool 角色：这条结果回答的调用 id（DSH ToolResultMessage.toolCallId）。
    #[serde(skip)]
    pub tool_call_id: Option<String>,
    /// 仅 tool 角色：工具调用是否失败（DSH isError，缺省 false）。
    #[serde(skip)]
    pub is_error: Option<bool>,
}

impl Message {
    /// 通用构造（新消息；id/source 由具体构造器给）。
    pub fn with_source(
        id: MessageId,
        role: Role,
        content: Vec<ContentBlock>,
        source: MessageSource,
    ) -> Self {
        Self {
            id,
            role,
            content,
            source,
            tool_call_id: None,
            is_error: None,
        }
    }

    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        Self::with_source(
            MessageId::default(),
            role,
            content,
            MessageSource::default(),
        )
    }

    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self::new(role, vec![ContentBlock::text(text)])
    }

    /// 系统提示消息（source=system-prompt）。
    pub fn system_prompt(text: impl Into<String>) -> Self {
        Self::new(Role::System, vec![ContentBlock::text(text)])
    }

    /// 用户消息（source=user）。
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self::new(Role::User, content)
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self::user(vec![ContentBlock::text(text)])
    }

    /// developer 消息（log-only 的工具增删记录；生产者来源由调用方给）。
    pub fn developer(content: Vec<ContentBlock>) -> Self {
        Self::new(Role::Developer, content)
    }

    /// 助手消息（source=model，provider/model 是产出路由）。
    pub fn assistant(
        content: Vec<ContentBlock>,
        _provider: impl Into<String>,
        _model: impl Into<String>,
    ) -> Self {
        Self::new(Role::Assistant, content)
    }

    /// 一条工具结果消息。
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::new(
            Role::Tool,
            vec![ContentBlock::ToolResult {
                tool_use_id: tool_use_id.into(),
                content: content.into(),
                is_error,
            }],
        )
    }

    pub fn is_empty(&self) -> bool {
        self.content.is_empty()
    }
}

/// 用户消息：角色固定在 Role::User 的消息。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserMessage(Message);

impl UserMessage {
    pub fn text(text: impl Into<String>) -> Self {
        Self(Message::user_text(text))
    }

    pub fn new(content: Vec<ContentBlock>) -> Self {
        Self(Message::user(content))
    }

    pub fn as_message(&self) -> &Message {
        &self.0
    }

    pub fn into_message(self) -> Message {
        self.0
    }
}

/// 一次请求可用的工具定义（模型可见的 schema）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

/// 一次请求的用量；厂商没给的项保持 None（显示为「未知」，不是 0）。
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

    /// 给人看的单行用量；厂商没给的项写「未知」，不写 0。
    pub fn summary(&self) -> String {
        let render = |label: &str, value: Option<u64>| match value {
            Some(tokens) => format!("{label} {tokens}"),
            None => format!("{label} 未知"),
        };
        [
            render("输入", self.input_tokens),
            render("输出", self.output_tokens),
            render("缓存读", self.cache_read_input_tokens),
            render("缓存写", self.cache_creation_input_tokens),
        ]
        .join(" · ")
    }
}

/// 推理强度。默认 High（spec 实现决定）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Effort {
    Off,
    Low,
    #[default]
    High,
    Max,
}

impl Effort {
    /// wire 上的取值（off、low、high、max）。
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Low => "low",
            Self::High => "high",
            Self::Max => "max",
        }
    }

    /// 解析 CLI 传入的取值；非法值返回 None。
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
    /// 模型正常说完（end_turn / stop_sequence）。
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
    Reasoning,
    ToolCall,
}

/// provider 无关的流事件。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamChunk {
    BlockStart { index: usize, block_type: BlockType },
    TextDelta { index: usize, text: String },
    ReasoningDelta { index: usize, text: String },
    BlockEnd { index: usize, block: ContentBlock },
    Usage(Usage),
    Finish(FinishReason),
}
