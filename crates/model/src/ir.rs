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
    /// 新建消息的 id：uuid v4（照 DSH createMessage 的 randomUUID）。
    pub fn generate() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn parse(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }
}

impl Default for MessageSource {
    fn default() -> Self {
        Self::User { rpc_id: None }
    }
}

impl From<&str> for MessageId {
    fn from(raw: &str) -> Self {
        Self(raw.to_owned())
    }
}

/// 一条消息的来源（DSH MessageSourceMap 的四个 kind）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MessageSource {
    /// 用户输入或注入的上下文。rpcId 是调用方的请求 id（T006.16b：重启后仍可按 id 去重）。
    User {
        #[serde(rename = "rpcId", default, skip_serializing_if = "Option::is_none")]
        rpc_id: Option<String>,
    },
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
    /// 压缩 checkpoint 的 user 消息（DSH compaction 的 compact-checkpoint 来源）。
    CompactCheckpoint {
        #[serde(rename = "compactionId")]
        compaction_id: String,
        #[serde(
            rename = "sourceCommandId",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        source_command_id: Option<String>,
    },
    /// 工作区指令注入的 user 消息（DSH agent-instructions 的来源，T006.10）：带类型来源、
    /// 完整基线 identity 与逐 scope 的 set/replace/remove 变更对象（内容不进 source）。
    AgentInstructions {
        form: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        baseline: bool,
        #[serde(
            rename = "baselineIdentity",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        baseline_identity: Option<String>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        changes: Vec<InstructionChange>,
    },
    /// 重启后回答已恢复问题的 user 消息（DSH user-questions 的 user-question-reply 来源，
    /// T006.13）：callId 指向那次 ask_user_question 调用。关闭不答不落回复，所以没有别的 outcome。
    UserQuestionReply {
        #[serde(rename = "callId")]
        call_id: String,
    },
    /// skill 目录注入的 user 消息（DSH tool-skill 的 skill-catalog 来源，T006.11）：
    /// 带类型目录（entries 是消息实际发布的摘要条目）；update=true 表示替换目录。
    SkillCatalog {
        form: String,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        update: bool,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        incomplete: bool,
        entries: Vec<SkillCatalogEntry>,
    },
    /// 用户显式 /name 调用注入的 user 消息（DSH tool-skill 的 skill-invocation 来源，T006.11b）：
    /// name 是用户点名的 skill，form 照 DSH 固定是 instructions。
    SkillInvocation { name: String, form: String },
}

/// skill 目录消息的一条持久条目（DSH SkillCatalogSource.entries 的 AIOS 形状）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SkillCatalogEntry {
    pub name: String,
    pub description: String,
}

/// 指令状态的变更动作（DSH AgentInstructionChange.action）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InstructionChangeAction {
    /// 新加载的文件（基线或更深目录）。
    Set,
    /// 文件内容变化后的替换。
    Replace,
    /// 文件消失/不再适用后的移除。
    Remove,
}

/// 一条指令变更对象（DSH AgentInstructionChange）：逻辑 scope + 展示路径 + 内容 digest。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstructionChange {
    pub action: InstructionChangeAction,
    pub scope: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub digest: Option<String>,
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
#[serde(tag = "type", rename_all = "kebab-case")]
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
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        Self::Text { text: text.into() }
    }

    /// 文本块的内容；非文本块返回 None。
    pub fn as_text(&self) -> Option<&str> {
        match self {
            Self::Text { text } => Some(text),
            Self::Reasoning { .. } | Self::ToolCall { .. } => None,
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
            Self::Text { .. } | Self::Reasoning { .. } => None,
        }
    }
}

/// 一条 provider 无关的消息（DSH MessageBase + 角色字段）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message {
    /// 稳定 id；序列化/续接/派生都保持不变。
    pub id: MessageId,
    pub role: Role,
    pub content: Vec<ContentBlock>,
    /// 生产者声明的来源。
    pub source: MessageSource,
    /// 仅 tool 角色：这条结果回答的调用 id（DSH ToolResultMessage.toolCallId）。
    #[serde(
        rename = "toolCallId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub tool_call_id: Option<String>,
    /// 仅 tool 角色：工具调用是否失败（DSH isError，缺省 false）。
    #[serde(rename = "isError", default, skip_serializing_if = "Option::is_none")]
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

    /// 通用构造（测试/派生用）：生成新 id，source 按角色推断；
    /// 生产消息用下面的角色构造器（assistant 要带真实路由）。
    pub fn new(role: Role, content: Vec<ContentBlock>) -> Self {
        let source = match role {
            Role::System => MessageSource::SystemPrompt,
            Role::Developer | Role::User => MessageSource::User { rpc_id: None },
            Role::Assistant => MessageSource::Model {
                provider: "deepseek".to_owned(),
                model: "deepseek-flash".to_owned(),
                replay_state: None,
            },
            Role::Tool => MessageSource::Tool {
                call_id: String::new(),
            },
        };
        Self::with_source(MessageId::generate(), role, content, source)
    }

    pub fn text(role: Role, text: impl Into<String>) -> Self {
        Self::new(role, vec![ContentBlock::text(text)])
    }

    /// 系统提示消息（source=system-prompt；空文本表示没有系统提示）。
    pub fn system_prompt(text: impl Into<String>) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::System,
            vec![ContentBlock::text(text)],
            MessageSource::SystemPrompt,
        )
    }

    /// 用户消息（source=user；注入类上下文也走这里，AIOS 暂无插件来源种类）。
    pub fn user(content: Vec<ContentBlock>) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::User,
            content,
            MessageSource::User { rpc_id: None },
        )
    }

    pub fn user_text(text: impl Into<String>) -> Self {
        Self::user(vec![ContentBlock::text(text)])
    }

    /// 带调用方请求 id 的用户消息（T006.16b 的 durable rpcId）。
    pub fn user_text_with_request_id(
        text: impl Into<String>,
        request_id: impl Into<String>,
    ) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::User,
            vec![ContentBlock::text(text)],
            MessageSource::User {
                rpc_id: Some(request_id.into()),
            },
        )
    }

    /// skill 目录注入消息（source=skill-catalog，T006.11 照 DSH tool-skill）。
    pub fn skill_catalog(
        text: impl Into<String>,
        update: bool,
        incomplete: bool,
        entries: Vec<SkillCatalogEntry>,
    ) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::User,
            vec![ContentBlock::text(text)],
            MessageSource::SkillCatalog {
                form: "catalog".to_owned(),
                update,
                incomplete,
                entries,
            },
        )
    }

    /// 用户显式 /name 调用注入的 skill 全文消息（source=skill-invocation，T006.11b 照 DSH
    /// tool-skill：claimed user 消息里的 /name 命中 user-invocable skill 时注入）。
    pub fn skill_invocation(text: impl Into<String>, name: String) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::User,
            vec![ContentBlock::text(text)],
            MessageSource::SkillInvocation {
                name,
                form: "instructions".to_owned(),
            },
        )
    }

    /// 重启后续答已恢复问题的 user 消息（source=user-question-reply，T006.13 照 DSH
    /// user-questions）：内容是一批答案的 JSON，callId 指向那次 ask_user_question 调用。
    pub fn user_question_reply(text: impl Into<String>, call_id: String) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::User,
            vec![ContentBlock::text(text)],
            MessageSource::UserQuestionReply { call_id },
        )
    }

    /// 工作区指令注入消息（source=agent-instructions，T006.10 照 DSH）。
    pub fn agent_instructions(
        text: impl Into<String>,
        baseline: bool,
        baseline_identity: Option<String>,
        changes: Vec<InstructionChange>,
    ) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::User,
            vec![ContentBlock::text(text)],
            MessageSource::AgentInstructions {
                form: "instructions".to_owned(),
                baseline,
                baseline_identity,
                changes,
            },
        )
    }

    /// developer 消息（log-only 的工具增删记录；AIOS 还没有生产者）。
    pub fn developer(content: Vec<ContentBlock>) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::Developer,
            content,
            MessageSource::User { rpc_id: None },
        )
    }

    /// 助手消息（source=model，provider/model 是产出路由）。
    pub fn assistant(
        content: Vec<ContentBlock>,
        provider: impl Into<String>,
        model: impl Into<String>,
    ) -> Self {
        Self::with_source(
            MessageId::generate(),
            Role::Assistant,
            content,
            MessageSource::Model {
                provider: provider.into(),
                model: model.into(),
                replay_state: None,
            },
        )
    }

    /// 一条工具结果消息（DSH ToolResultMessage：callId/isError 在消息级，content 是结果块）。
    pub fn tool_result(
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        Self::tool_result_with_id(MessageId::generate(), tool_use_id, content, is_error)
    }

    /// 带指定 id 的工具结果消息（修复/日志回放用；id 必须由调用方确定）。
    pub fn tool_result_with_id(
        id: MessageId,
        tool_use_id: impl Into<String>,
        content: impl Into<String>,
        is_error: bool,
    ) -> Self {
        let tool_use_id = tool_use_id.into();
        Self {
            id,
            role: Role::Tool,
            content: vec![ContentBlock::text(content)],
            source: MessageSource::Tool {
                call_id: tool_use_id.clone(),
            },
            tool_call_id: Some(tool_use_id),
            is_error: if is_error { Some(true) } else { None },
        }
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

    /// 带 requestId 的用户消息（T006.16b：服务发消息的 durable rpcId）。
    pub fn text_with_request_id(text: impl Into<String>, request_id: impl Into<String>) -> Self {
        Self(Message::user_text_with_request_id(text, request_id))
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
    // 字段名照 DSH TokenUsage（packages/llm/llm/src/types.ts:182-196）。
    #[serde(rename = "inputTokens", skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    #[serde(rename = "outputTokens", skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    #[serde(rename = "cacheReadTokens", skip_serializing_if = "Option::is_none")]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(rename = "cacheWriteTokens", skip_serializing_if = "Option::is_none")]
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

/// 流的结束原因；JSON 照 DSH FinishReasonMap（`{"kind":"stop"}` 等，types.ts:159-172）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum FinishReason {
    /// 模型正常说完（end_turn / stop_sequence）。
    Stop,
    /// 模型要调工具。
    ToolCalls,
    /// 撞上输出上限。
    MaxTokens,
}

/// 内容块类型（流事件里只带类型，不带内容）；JSON 是 DSH 的块类型名（`tool-call`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BlockType {
    Text,
    Reasoning,
    ToolCall,
}

/// provider 无关的流事件；JSON 照 DSH StreamChunk（types.ts:496-508）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum StreamChunk {
    BlockStart {
        index: usize,
        #[serde(rename = "blockType")]
        block_type: BlockType,
    },
    TextDelta {
        index: usize,
        text: String,
    },
    ReasoningDelta {
        index: usize,
        text: String,
    },
    /// 工具调用参数的增量（DSH tool-call-delta：block-start 时带 id/name、
    /// argumentsDelta 为空，之后每个 input_json_delta 一帧；translate.ts:82-85,130）。
    ToolCallDelta {
        index: usize,
        id: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        name: Option<String>,
        #[serde(rename = "argumentsDelta")]
        arguments_delta: String,
    },
    BlockEnd {
        index: usize,
        block: ContentBlock,
    },
    Usage {
        usage: Usage,
    },
    Finish {
        reason: FinishReason,
    },
}

// 测试直接展开在 ir 模块里：票面 C5 的测试名是 model::ir::<test>（不加 tests 子模块）。
#[cfg(test)]
include!("ir_tests.rs");
