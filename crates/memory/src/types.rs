use serde::{Deserialize, Serialize};

/// 谁在写：用户、Kind、做梦。校验在 store，不靠提示词。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Actor {
    User,
    Kind,
    Dream,
}

/// 记忆的层；日记是单独表，但搜索/导出时也用它。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Layer {
    AboutYou,
    Entry,
    Diary,
    Note,
}

impl Layer {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AboutYou => "about_you",
            Self::Entry => "entry",
            Self::Diary => "diary",
            Self::Note => "note",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "about_you" => Some(Self::AboutYou),
            "entry" => Some(Self::Entry),
            "diary" => Some(Self::Diary),
            "note" => Some(Self::Note),
            _ => None,
        }
    }
}

/// 分量：有约束力 / 可错。只有用户定下来或用户自己说的才是 binding。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Binding {
    Binding,
    Fallible,
}

impl Binding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Binding => "binding",
            Self::Fallible => "fallible",
        }
    }
}

/// 范围。这版只写 personal；project/role 留字段。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Personal,
    Project,
    Role,
}

impl Scope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Personal => "personal",
            Self::Project => "project",
            Self::Role => "role",
        }
    }
}

/// 记忆条目的只读视图（页面/工具用）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryItem {
    pub id: String,
    pub layer: Layer,
    pub binding: Binding,
    pub scope: Scope,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub source_kind: String,
    pub source_ref: String,
    pub status: String,
    pub current_revision: u64,
    pub updated_at: i64,
    pub pinned: bool,
}

/// 一条只追加的修订。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionItem {
    pub memory_id: String,
    pub revision: u64,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub reason: String,
    pub dream_run_id: Option<String>,
    pub undo_of: Option<u64>,
    pub created_at: i64,
}

/// 一条日记。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiaryEntry {
    pub id: String,
    pub day: String,
    pub seq: u64,
    pub text: String,
    pub source_ref: String,
    pub created_at: i64,
}

/// 搜索命中：记忆或对话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SearchHit {
    Memory(MemoryItem),
    Conversation {
        conversation_id: String,
        seq: u64,
        turn: u64,
        text: String,
    },
}

/// 一次写入携带的来源/身份。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WriteContext {
    pub actor: Actor,
    pub dream_run: Option<String>,
}

impl WriteContext {
    pub fn user() -> Self {
        Self {
            actor: Actor::User,
            dream_run: None,
        }
    }
    pub fn kind() -> Self {
        Self {
            actor: Actor::Kind,
            dream_run: None,
        }
    }
    pub fn dream(run: impl Into<String>) -> Self {
        Self {
            actor: Actor::Dream,
            dream_run: Some(run.into()),
        }
    }
}

/// 写入动作。服务层只通过这些动作改库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteOp {
    /// 用户说「记住」：关于你，binding。
    Remember {
        title: String,
        summary: String,
        body: String,
        source_kind: String,
        source_ref: String,
    },
    /// Kind 自己记：note，fallible。
    Note {
        title: String,
        summary: String,
        body: String,
        source_kind: String,
        source_ref: String,
    },
    /// 压缩前/用户说记一笔：日记只追加。
    Diary { text: String, source_ref: String },
    /// 用户/页面更正一条已有条目：新修订。
    Correct {
        id: String,
        title: String,
        summary: String,
        body: String,
        reason: String,
    },
    /// 删到隔离区（7 天内能恢复）。
    Delete { id: String, reason: String },
    /// 从隔离区恢复。
    Restore { id: String },
    /// 立即彻底删；留无明文 tombstone。
    Forget { id: String },
    /// 隔离区到期清理（内部/启动）。
    PurgeExpired,
    /// 把 Kind 笔记定成有约束力的条目。
    Pin { id: String },
    /// 做梦一次应用；changes 由 worker 生成。
    DreamApply {
        run: DreamRunInput,
        changes: Vec<DreamChangeInput>,
    },
    /// 撤销一处/整次做梦；only 是 memory id。
    DreamUndo {
        run_id: String,
        only: Option<String>,
    },
    /// 对话索引一条；按 (conversation_id, seq) 幂等。
    IndexConversation {
        conversation_id: String,
        seq: u64,
        turn: u64,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamRunInput {
    pub id: String,
    pub model: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub outcome: String,
    pub report_json: String,
    pub usage_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DreamAction {
    Merge,
    Correct,
    Retire,
    NewNote,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamChangeInput {
    pub memory_id: Option<String>,
    pub title: String,
    pub summary: String,
    pub body: String,
    pub action: DreamAction,
    pub reason: String,
}

/// 写入结果：新建/更新的条目、日记、或做梦撤销报告。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct WriteOutcome {
    pub item: Option<MemoryItem>,
    pub diary: Option<DiaryEntry>,
    pub dream_applied: usize,
    pub dream_skipped: Vec<String>,
    pub purged: usize,
}

/// 打开后的一次全量视图；version 随每次写递增，服务据此推送。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryView {
    pub version: u64,
    pub about_you: Vec<MemoryItem>,
    pub entries: Vec<MemoryItem>,
    pub notes: Vec<MemoryItem>,
    pub diary: Vec<DiaryEntry>,
    pub quarantine: Vec<MemoryItem>,
    pub settings: Vec<(String, String)>,
}
