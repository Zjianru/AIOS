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
    /// 成立时间；迁移旧行 = created_at。
    pub valid_from: i64,
    pub valid_until: Option<i64>,
    /// replaced / user_deadline / deleted；None = 仍有效。
    pub invalid_reason: Option<String>,
    pub superseded_by: Option<String>,
    pub possibly_stale: bool,
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
    /// 这一版是谁改的（user/kind/dream；旧库迁移的行是 system）。
    pub actor: String,
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
    Diary(DiaryEntry),
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

/// 做梦/观察的设置。默认值照 spec：开着、没许可、24 小时、5 个新回合或报回。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamSettings {
    pub enabled: bool,
    /// 用户给过观察许可（第一次跑之前要有一张许可卡，允许后才读对话）。
    pub permit: bool,
    pub interval_hours: u64,
    pub min_events: u64,
    /// 上次成功的时间（毫秒）；0 = 从没成功过。
    pub last_success_at: i64,
    /// 上次尝试的时间（毫秒）；失败后退一段间隔再试，避免每轮都重试。
    pub last_attempt_at: i64,
    /// 上次成功读到的 Kind 会话 seq；下次从这里往后读。
    pub last_read_seq: u64,
    /// 许可卡上次发的时间；0 = 没发过。
    pub permit_card_at: i64,
}

impl Default for DreamSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            permit: false,
            interval_hours: 24,
            min_events: 5,
            last_success_at: 0,
            last_attempt_at: 0,
            last_read_seq: 0,
            permit_card_at: 0,
        }
    }
}

/// 设置补丁：只写给出的项。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamSettingsPatch {
    pub enabled: Option<bool>,
    pub permit: Option<bool>,
    pub interval_hours: Option<u64>,
    pub min_events: Option<u64>,
    pub last_success_at: Option<i64>,
    pub last_attempt_at: Option<i64>,
    pub last_read_seq: Option<u64>,
    pub permit_card_at: Option<i64>,
}

/// 一次做梦的记录（明细在 dream_change）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamRun {
    pub id: String,
    pub started_at: i64,
    pub ended_at: i64,
    pub outcome: String,
    pub model: String,
    pub usage_json: String,
    pub report_json: String,
    pub error: String,
}

/// 做梦改过的一处；撤销按 before/after revision 判断用户有没有后来改过。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DreamChange {
    pub run_id: String,
    pub memory_id: String,
    pub before_revision: u64,
    pub after_revision: u64,
    pub action: String,
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
    /// duplicate 时命中的旧条目 id；新条目为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duplicate_of: Option<String>,
    /// 新建条目的关系目标（supplement/replaces 边指向的那条）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub related: Option<String>,
    /// 这次写入产生的待决卡（替换卡 / 关于你草稿卡）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub card: Option<MemoryCardRecord>,
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

/// 钥匙种类：按人/项目/命令/事件/关键词召回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKeyKind {
    Keyword,
    Path,
    Command,
    Event,
    Entity,
}

impl MemoryKeyKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyword => "keyword",
            Self::Path => "path",
            Self::Command => "command",
            Self::Event => "event",
            Self::Entity => "entity",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "keyword" => Some(Self::Keyword),
            "path" => Some(Self::Path),
            "command" => Some(Self::Command),
            "event" => Some(Self::Event),
            "entity" => Some(Self::Entity),
            _ => None,
        }
    }
}

/// 一条记忆的钥匙；同一 key_no 的几把是「全部满足」的条件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryKey {
    pub key_no: u32,
    pub kind: MemoryKeyKind,
    pub value: String,
}

/// 一条出处；旧库迁移不造 source 行，所以 quote 为空表示「只有引用、没留原话」。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemorySource {
    pub source_kind: String,
    pub source_ref: String,
    pub quote: Option<String>,
    pub added_at: i64,
}

/// 一次 schema 迁移的报告；只有真的从旧库迁上来时才有。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryMigrationReport {
    pub from_version: u64,
    pub to_version: u64,
    /// 迁移前的备份路径；没有备份（新库/非 v2）为空。
    pub backup: Option<String>,
    /// 旧 memory 行里没有原文的行数（旧库没有 quote 表，迁移不伪造 quote）。
    pub missing_original: u64,
}

/// 一次带钥匙/关系的写入（深接口；服务层只经这里写记忆）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryWrite {
    pub op: WriteOp,
    pub keys: Vec<MemoryKey>,
    pub relation: Option<RelationDecision>,
    /// 成立时间；None = 用现在。迁移/回填之外都写现在。
    pub valid_from: Option<i64>,
    /// 用户原话/按来源核过的原话；来源引用另由 op 带。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_quote: Option<String>,
}

impl MemoryWrite {
    pub fn new(op: WriteOp) -> Self {
        Self {
            op,
            keys: Vec::new(),
            relation: None,
            valid_from: None,
            source_quote: None,
        }
    }
}

/// 写入前对旧条目的关系判断。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RelationDecision {
    Duplicate { target: String },
    Contradict { target: String, reason: String },
    Supplement { target: String },
    Independent,
}

/// 关系候选：程序按范围 + 钥匙捞出来的旧条目，交给模型判关系。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RelationCandidate {
    pub id: String,
    pub title: String,
    pub summary: String,
}

/// 一条关系边：supplement（补充）/ replaces（替换）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryRelation {
    pub related_id: String,
    pub kind: String,
}

/// 一条待决记忆卡（替换卡 / 关于你草稿卡）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemoryCardRecord {
    pub card_ref: String,
    pub kind: String,
    pub status: String,
    pub payload: serde_json::Value,
}

/// 关于你的一处草稿改动（程序核过原话后才进卡）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AboutEdit {
    /// add / replace / delete
    pub op: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub about_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub old: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// 关于你卡里的 payload。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AboutDraftPayload {
    pub quote: String,
    pub edits: Vec<AboutEdit>,
}
