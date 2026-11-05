//! T006.16a：与载体无关的服务宿主（接缝 3）——调用对象 + 事件流；Web 适配器在 T006.17。
//!
//! DSH 对照：packages/api/session-controller/src/index.ts 的 Host 方法面（prompt/cancel/events/state
//! 等；见 T006/evidence/T006.16a-dsh-compare.log 的引用校正）。本票只做「送调用/收事件」的接缝与
//! 第一版方法/事件形状，不起 HTTP/WS（T006.17），不接页面（T008）。
//!
//! 对象固定 Kind 根会话：根会话实体在 T006.16b 落，本票宿主绑定一个会话、Object 用 session id。

pub mod fx;
pub mod quota;
mod root;
pub mod settings;
pub mod stats;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent::{Agent, SendAdmission};
use model::UserMessage;
pub use root::{
    KeyringChanged, ModelFactory, ROOT_STATE_FILE, RootError, RootSession, RootSessionOptions,
    RootStatus, open_root_session, session_model_selection,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// 服务事件流容量（订阅者落后时用 Lagged；重新跟随在 T006.16b）。
const SERVICE_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// 服务对象：固定 Kind 根会话（16b 换成根会话实体）；第一版用会话 id 表达。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// 投递方式（照 T006.2a 的 Delivery；服务协议里用稳定的 kebab-case 名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeliveryMode {
    /// 下一轮：这一轮结束后自动开新一轮。
    FollowUp,
    /// 下一步：下一个 step 的请求里带上它，并唤醒空闲的 loop。
    Steer,
    /// 下一步：下一个 step 的请求里带上它，但不唤醒空闲的 loop。
    Inject,
}

impl From<DeliveryMode> for agent::Delivery {
    fn from(mode: DeliveryMode) -> Self {
        match mode {
            DeliveryMode::FollowUp => agent::Delivery::FollowUp,
            DeliveryMode::Steer => agent::Delivery::Steer,
            DeliveryMode::Inject => agent::Delivery::Inject,
        }
    }
}

/// 一次调用（载体把线上请求解码成它；Web 适配器在 T006.17）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case")]
pub enum Call {
    Send(SendRequest),
    Cancel(CancelRequest),
    Answer(AnswerRequest),
    DismissQuestions(DismissQuestionsRequest),
    Compact(CompactRequest),
    Status(StatusRequest),
    Follow(FollowRequest),
    Events(EventsRequest),
    /// T007.2：再检查一次根会话（修好 missing/corrupt/busy 之后回到对话）。
    Recheck(RecheckRequest),
    ModelSettings(ModelSettingsRequest),
    ConnectionSave(ConnectionSaveRequest),
    ConnectionDelete(ConnectionDeleteRequest),
    /// T008.3 增补：只改 endpoint/model，保留 key。
    ConnectionUpdate(ConnectionUpdateRequest),
    /// T008.3 增补：撤销 key，连接保留。
    ConnectionRevoke(ConnectionRevokeRequest),
    /// T008.3：验证连接（最小请求）；结果写进注册器。
    ConnectionVerify(ConnectionVerifyRequest),
    /// T012.5：获取可用模型（目录 + 厂商列表）。
    AvailableModels(AvailableModelsRequest),
    KeyringRelocate(KeyringRelocateRequest),
    KeyringReopen(KeyringReopenRequest),
    KeyringRestart(KeyringRestartRequest),
    Issues(IssuesRequest),
    FirstUse(FirstUseRequest),
    /// T008.3：选模型（下一轮生效，当前轮不换）。
    SessionModelSelect(SessionModelSelectRequest),
    /// T012.8b：查询连接余额/套餐余量。
    ConnectionQuota(ConnectionQuotaRequest),
}

/// 调用的结果（与 Call 一一对应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case")]
pub enum Response {
    Send(SendResponse),
    Cancel(CancelResponse),
    Answer(AnswerResponse),
    DismissQuestions(DismissQuestionsResponse),
    Compact(CompactResponse),
    Status(StatusResponse),
    Follow(FollowResponse),
    Events(EventsResponse),
    Recheck(RecheckResponse),
    ModelSettings(ModelSettingsResponse),
    ConnectionSave(ConnectionSaveResponse),
    ConnectionDelete(ConnectionDeleteResponse),
    ConnectionUpdate(ConnectionUpdateResponse),
    ConnectionRevoke(ConnectionRevokeResponse),
    ConnectionVerify(ConnectionVerifyResponse),
    /// T012.8b：连接余额/套餐余量。
    ConnectionQuota(ConnectionQuotaResponse),
    AvailableModels(AvailableModelsResponse),
    KeyringRelocate(KeyringRelocateResponse),
    KeyringReopen(KeyringReopenResponse),
    KeyringRestart(KeyringRestartResponse),
    Issues(IssuesResponse),
    FirstUse(FirstUseResponse),
    SessionModelSelect(SessionModelSelectResponse),
}

/// 发消息（DSH session-controller 的 prompt）。requestId 是幂等键：同一 id 只进一次。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub content: String,
    #[serde(default = "default_delivery")]
    pub delivery: DeliveryMode,
}

fn default_delivery() -> DeliveryMode {
    DeliveryMode::FollowUp
}

/// 发消息的受理回执（重复 requestId 返回原始受理，duplicate=true）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub accepted: bool,
    pub duplicate: bool,
}

/// 取消当前轮（不丢 pending inbox，照 T006.2a 的 cancel）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CancelRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelResponse {
    pub cancelled: bool,
}

/// 一条答案（接 T006.13 的 {id, selected[], custom?}）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceAnswer {
    #[serde(rename = "questionId")]
    pub question_id: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// 回答整批（一次调用覆盖一批问题；逐题走 Agent::answer，全部被接受才算成功）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub answers: Vec<ServiceAnswer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AnswerResponse {
    pub applied: usize,
}

/// 关掉/取消待答问题（T006.16c；DoD5 的「关掉不答」）。一次撤掉当前全部待答问题：
/// 等待中的批以 DSH ASK_ABORTED 工具错误收尾、本轮继续；重启恢复的批先补写已收答案，
/// 再写 question/cancelled 并移除。DSH 没有对应的 Remote 方法（关闭面板不发请求，靠
/// timed wait 超时；见 user-questions README 第 35、40 行与 T006.16c-dsh-compare.log），
/// 这里是按 T006.13 撤销语义的适配，不假装与 DSH 相同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct DismissQuestionsRequest {}

/// 关掉不答的回执：是否确实有待答问题被撤掉（没有待答问题时 false）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DismissQuestionsResponse {
    pub dismissed: bool,
}

/// 手动压缩（DSH command-compact）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CompactRequest {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "commandId")]
    pub command_id: Option<String>,
}

/// 压缩计数（有可压缩历史才 Some）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactCounts {
    #[serde(rename = "shadowedItems")]
    pub shadowed_items: usize,
    #[serde(rename = "shadowedTokens")]
    pub shadowed_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CompactResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted: Option<CompactCounts>,
}

/// 被动状态（DSH state 的第一版子集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StatusRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub object: Object,
    pub running: bool,
    #[serde(rename = "pendingQuestions")]
    pub pending_questions: usize,
    /// T012.8：会话统计（轮/步/token/缓存/上下文/花费）；裸宿主为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<stats::SessionStats>,
    /// T012.8：USD→CNY 汇率与拉到时间；裸宿主为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<fx::FxInfo>,
    /// T012.8：币种（settings.json：usd/cny）；裸宿主为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

/// follow（DSH :607）：带上调用方最后看到的日志序号，服务补齐之后的事件。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FollowRequest {
    /// 调用方已持有的最后序号（0 = 从头补齐）。
    #[serde(default, rename = "lastSeq")]
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FollowResponse {
    /// seq > last_seq 的事件（按日志顺序、连续、不重复）。
    pub events: Vec<ServiceEvent>,
    /// 补齐后的最后序号（events 为空时等于请求的 last_seq）。
    #[serde(rename = "lastSeq")]
    pub last_seq: u64,
}

/// events（DSH :540）：往回翻一页日志，按轮对齐、有预算（默认 50/2/500）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct EventsRequest {
    /// 页的末端（不含本条）；省略 = 当前日志末端之后。
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "beforeSeq")]
    pub before_seq: Option<u64>,
    /// 消息条数上限（user/message + assistant/message），默认 500。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "maxMessages"
    )]
    pub max_messages: Option<usize>,
    /// 至少要有多少条消息（轮对齐下限），默认 50。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "minMessages"
    )]
    pub min_messages: Option<usize>,
    /// 至少跨过多少个轮起点（轮对齐下限），默认 2。
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "minTurns")]
    pub min_turns: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct RecheckRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecheckResponse {
    pub root: RootState,
    /// T009.5：根不可用时没有 ready 帧，页面靠这里的 host.home 缩写说明页里的路径。
    #[serde(default)]
    pub host: HostLocation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsResponse {
    /// 这一页事件（按日志顺序；页首通常是 turn/start，除非预算或日志耗尽先到）。
    pub events: Vec<ServiceEvent>,
    /// 还有更早的事件（下一轮用 beforeSeq = 页首 seq 往回翻，不含本条）。
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

/// 失败原因分类（T007.2）：页面失败块按这个分类写文案，不按 code 猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureCause {
    /// 凭证问题（401 / 没配 key）——T008 的 Keyring 另算。
    Credential,
    /// 模型侧失败（重试用完/超时/服务错误等）。
    Model,
    /// T008.4：Keyring 读不出/无法签发；页面失败块说 Keyring 原因、指去设置。
    Keyring,
}

/// 服务事件：严格来自会话日志事实（seq 连续、type 是日志事件类型、data 是事件字段）。
/// cause 是 T007.2 只增的派生字段：只有 turn/end 的 error 事件带，取值来自日志里的 code，
/// 页面拿它选失败块文案，不在页面上推算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEvent {
    pub seq: u64,
    /// 日志事件发生时间（unix 毫秒；页面工作过程开始时间从这里取）。
    pub time: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<FailureCause>,
}

/// T008.3：设置页读模型连接（空请求体）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettingsRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSettingsResponse {
    pub settings: credentials::KeyringView,
}

/// T008.3：存/换连接；apiKey 只在请求体里，任何响应不带。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ConnectionSaveRequest {
    pub id: String,
    pub name: String,
    pub model: String,
    pub endpoint: String,
    #[serde(rename = "apiKey")]
    pub api_key: String,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub models: Option<Vec<credentials::ModelRow>>,
    #[serde(default)]
    pub price_rate: Option<f64>,
    #[serde(default)]
    pub auth_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionSaveResponse {
    pub connection: credentials::ConnectionInfo,
}

/// T012.5：获取某连接的可用模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AvailableModelsRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableModelsResponse {
    pub models: Vec<model::catalog::ModelInfo>,
    /// 厂商 /v1/models 是否成功；失败时只给目录/用户模型。
    pub live: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionDeleteRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionDeleteResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyringRelocateRequest {
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringRelocateResponse {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyringReopenRequest {
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringReopenResponse {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyringRestartRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeyringRestartResponse {
    pub settings: credentials::KeyringView,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssuesRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IssuesResponse {
    #[serde(rename = "issuedToday")]
    pub issued_today: u64,
    pub issues: Vec<credentials::IssueRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FirstUseRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FirstUseResponse {
    #[serde(rename = "firstUse")]
    pub first_use: bool,
    #[serde(rename = "keyFile")]
    pub key_file: credentials::KeyFileInfo,
}

/// T008.3：验证一个连接（空 = 用注册器里的 endpoint/key 发最小请求）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionVerifyRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionVerifyResponse {
    pub connection: credentials::ConnectionInfo,
}

/// T012.8b：查询连接余额/套餐余量（查不到不报错，字段为空）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConnectionQuotaRequest {
    pub id: String,
}

/// 余额（厂商币种）与套餐余量（MiniMax：5 小时窗口/每周剩余百分比）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionQuotaResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<quota::BalanceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<quota::PlanQuota>,
    /// 查询时间（Unix 秒）。
    #[serde(rename = "checkedAt")]
    pub checked_at: u64,
}

/// T008.3/T012.7：选模型（连接、模型、想要的档）；只影响下一轮（D26）。
///
/// 三个字段都可选（协议只增）：None = 保持当前值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SessionModelSelectRequest {
    /// 连接 id（Keyring 里的连接）；None = 保持当前连接。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection: Option<String>,
    /// 模型 id；None = 保持当前模型。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// 想要的思考档（模型自己的字符串）；None = 保持当前。
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        rename = "effortWanted"
    )]
    pub effort_wanted: Option<String>,
}

/// 选择结果：生效的目标选择（下一轮开始用）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SessionModelSelectResponse {
    pub connection: String,
    pub model: String,
    #[serde(rename = "effortWanted")]
    pub effort_wanted: String,
}

/// T008.3 增补：编辑连接，只改 endpoint/model（不换 key）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct ConnectionUpdateRequest {
    pub id: String,
    pub endpoint: String,
    pub model: String,
    #[serde(default)]
    pub preset: Option<String>,
    #[serde(default)]
    pub models: Option<Vec<credentials::ModelRow>>,
    #[serde(default)]
    pub price_rate: Option<f64>,
    #[serde(default)]
    pub auth_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionUpdateResponse {
    pub connection: credentials::ConnectionInfo,
}

/// T008.3 增补：撤销 key（连接保留，之后签发失败）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionRevokeRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionRevokeResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 服务协议版本（T007.2 起）：只增不改的对外面每次扩展都记在这里，Web ready 帧里带上。
pub const PROTOCOL_VERSION: u64 = 4;

/// 根会话不可用的种类（T007.2）：服务照常起，ready 报出来，方法一律拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RootUnavailableKind {
    /// 状态文件记过会话 id，但会话日志不见了。
    Lost,
    /// 状态文件损坏 / 版本未知。
    Corrupt,
    /// 另一个进程正持有会话写锁。
    Busy,
    /// 其他打开失败（IO/底层错误）。
    Other,
}

/// 根会话状态（T007.2）：ready 里原样给页面，用于五种说明页。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum RootState {
    Ok,
    Unavailable {
        kind: RootUnavailableKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        line: Option<u64>,
        message: String,
    },
}

impl RootState {
    /// 把打开根会话的错误分类成页面状态（只增：error 的 Debug/说明仍保留）。
    pub fn from_root_error(error: &RootError) -> Self {
        match error {
            RootError::StateCorrupt { path, line, .. } => RootState::Unavailable {
                kind: RootUnavailableKind::Corrupt,
                path: Some(path.clone()),
                line: *line,
                message: error.to_string(),
            },
            RootError::RecordedSessionMissing { path, .. } => RootState::Unavailable {
                kind: RootUnavailableKind::Lost,
                path: Some(path.clone()),
                line: None,
                message: error.to_string(),
            },
            RootError::WriterHeld { .. } => RootState::Unavailable {
                kind: RootUnavailableKind::Busy,
                path: None,
                line: None,
                message: error.to_string(),
            },
            RootError::Agent(_) | RootError::Io(_) => RootState::Unavailable {
                kind: RootUnavailableKind::Other,
                path: None,
                line: None,
                message: error.to_string(),
            },
        }
    }
}

/// 上下文占用（T007.2）：服务从日志里的 usage + 模型配置的窗口算，页面不推算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub used: u64,
    pub window: u64,
}

/// T009.5：宿主机的位置（ready 帧的 host）：页面只用 home 把路径缩写成 ~/…。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HostLocation {
    /// 宿主进程的 HOME（没有就不给；页面只在有值时缩写）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

/// ready/status 里给页面的宿主信息（协议版本 + 根会话状态 + 上下文占用 + 宿主位置）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HostInfo {
    #[serde(rename = "protocolVersion")]
    pub protocol_version: u64,
    #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub root: RootState,
    pub context: ContextUsage,
    /// T009.5：ready 帧加 host: {home}；不加 clientId。
    pub host: HostLocation,
}

/// 服务错误（协议层错误码的第一版；Web 适配器在 17 映射成线上错误）。
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Agent(#[from] agent::AgentError),
    #[error("回答被拒绝的问题：{0}")]
    AnswerRejected(String),
    /// T007.2：根会话不可用，方法一律拒绝；页面用 ready 里的状态显示说明页。
    #[error("Kind 的根会话不可用")]
    RootUnavailable(RootState),
    #[error(transparent)]
    Credentials(#[from] credentials::KeyringError),
    #[error("设置接口只能由根宿主提供")]
    SettingsOnBareHost,
    #[error("路径不合法：{0}")]
    InvalidPath(String),
}

/// T009.1：服务的结构化错误码（DSH remote-error 形状：`<namespace>/<kind>`，小写连字符）。
///
/// 码表只在这一处；页面按码处理，说明只进监控/诊断。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceErrorCode {
    AgentInternal,
    QuestionAnswerRejected,
    RootUnavailable,
    KeyringKeyFileMissing,
    KeyringUndecryptable,
    KeyringCorrupt,
    KeyringIo,
    ConnectionMissing,
    ConnectionInvalid,
    SettingsUnavailable,
    RequestInvalidPath,
    RequestInvalidJson,
}

impl ServiceErrorCode {
    /// 码表全集（覆盖检查用：多一个少一个都要失败）。
    pub const ALL: [ServiceErrorCode; 12] = [
        ServiceErrorCode::AgentInternal,
        ServiceErrorCode::QuestionAnswerRejected,
        ServiceErrorCode::RootUnavailable,
        ServiceErrorCode::KeyringKeyFileMissing,
        ServiceErrorCode::KeyringUndecryptable,
        ServiceErrorCode::KeyringCorrupt,
        ServiceErrorCode::KeyringIo,
        ServiceErrorCode::ConnectionMissing,
        ServiceErrorCode::ConnectionInvalid,
        ServiceErrorCode::SettingsUnavailable,
        ServiceErrorCode::RequestInvalidPath,
        ServiceErrorCode::RequestInvalidJson,
    ];

    /// 线上的码名。
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentInternal => "agent/internal",
            Self::QuestionAnswerRejected => "question/answer-rejected",
            Self::RootUnavailable => "root/unavailable",
            Self::KeyringKeyFileMissing => "keyring/key-file-missing",
            Self::KeyringUndecryptable => "keyring/undecryptable",
            Self::KeyringCorrupt => "keyring/corrupt",
            Self::KeyringIo => "keyring/io",
            Self::ConnectionMissing => "connection/missing",
            Self::ConnectionInvalid => "connection/invalid",
            Self::SettingsUnavailable => "settings/unavailable",
            Self::RequestInvalidPath => "request/invalid-path",
            Self::RequestInvalidJson => "request/invalid-json",
        }
    }
}

impl ServiceError {
    /// T009.1：这个错误在 wire 上的码。
    pub fn code(&self) -> ServiceErrorCode {
        match self {
            Self::Agent(_) => ServiceErrorCode::AgentInternal,
            Self::AnswerRejected(_) => ServiceErrorCode::QuestionAnswerRejected,
            Self::RootUnavailable(_) => ServiceErrorCode::RootUnavailable,
            Self::Credentials(error) => match error {
                credentials::KeyringError::KeyFileMissing(_) => {
                    ServiceErrorCode::KeyringKeyFileMissing
                }
                credentials::KeyringError::Crypto(_) => ServiceErrorCode::KeyringUndecryptable,
                credentials::KeyringError::Corrupt(_) => ServiceErrorCode::KeyringCorrupt,
                credentials::KeyringError::Io(_) => ServiceErrorCode::KeyringIo,
                credentials::KeyringError::MissingConnection(_) => {
                    ServiceErrorCode::ConnectionMissing
                }
                credentials::KeyringError::InvalidConnection(_) => {
                    ServiceErrorCode::ConnectionInvalid
                }
            },
            Self::SettingsOnBareHost => ServiceErrorCode::SettingsUnavailable,
            Self::InvalidPath(_) => ServiceErrorCode::RequestInvalidPath,
        }
    }
}

/// T009.1：/api 的错误体（DSH RemoteError 的线上形状）：码 + 诊断 + 可选细节。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ServiceErrorBody {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

impl ServiceErrorBody {
    /// T009.1：码 + 诊断 + 按码定的细节（root/unavailable 带 root 状态与协议版本）。
    pub fn from_service_error(error: &ServiceError) -> Self {
        let details = match error {
            ServiceError::RootUnavailable(root) => Some(serde_json::json!({
                "root": root,
                "protocolVersion": PROTOCOL_VERSION,
            })),
            _ => None,
        };
        Self {
            code: error.code().as_str().to_owned(),
            message: error.to_string(),
            details,
        }
    }
}

/// 与载体无关的服务宿主：绑定一个 Agent/会话，调用经 dispatch 进入，事件从广播流出。
pub struct ServiceHost {
    agent: Agent,
    events: broadcast::Sender<ServiceEvent>,
    /// 串行化发送受理：并发同 requestId 时一次进、其余拿到 durable 判重（T006.16b）。
    send_lock: tokio::sync::Mutex<()>,
    forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl ServiceHost {
    /// 建宿主并开始把 Agent 的日志事件转发成服务事件（seq/type/data 都来自日志事实）。
    pub fn new(agent: Agent) -> Self {
        let (events, _) = broadcast::channel(SERVICE_EVENT_CHANNEL_CAPACITY);
        let mut source = agent.subscribe_session_events();
        let forward_tx = events.clone();
        let forward = tokio::spawn(async move {
            loop {
                match source.recv().await {
                    Ok(event) => {
                        let _ = forward_tx.send(service_event(&event));
                    }
                    // 订阅者落后/重连的重新跟随在 T006.16b；转发端自己不能停。
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        Self {
            agent,
            events,
            send_lock: tokio::sync::Mutex::new(()),
            forward: Mutex::new(Some(forward)),
        }
    }

    /// T008.3：选模型（下一轮生效；正在跑的请求不受影响）。
    pub fn select_model(&self, model: String) {
        self.agent.select_model(model);
    }

    /// T012.7：宿主绑定的 Agent（会话选择写日志/判断 busy；调用方不要长期持有）。
    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// 服务对象（第一版：会话 id；16b 换成 Kind 根会话实体）。
    pub fn object(&self) -> Object {
        Object {
            session_id: self.agent.id().as_str().to_owned(),
        }
    }

    /// 事件订阅端（载体每连接一个）；协议方法 events 见下方同名 RPC。
    pub fn subscribe_events(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }

    /// 唯一的调用入口：载体把线上请求解码成 Call，结果编码回 Response。
    pub async fn dispatch(&self, call: Call) -> Result<Response, ServiceError> {
        match call {
            Call::Send(request) => self.send(request).await.map(Response::Send),
            Call::Cancel(request) => self.cancel(request).await.map(Response::Cancel),
            Call::Answer(request) => self.answer(request).await.map(Response::Answer),
            Call::DismissQuestions(request) => self
                .dismiss_questions(request)
                .await
                .map(Response::DismissQuestions),
            Call::Compact(request) => self.compact(request).await.map(Response::Compact),
            Call::Status(request) => self.status(request).await.map(Response::Status),
            Call::Follow(request) => self.follow(request).await.map(Response::Follow),
            Call::Events(request) => self.events(request).await.map(Response::Events),
            // T007.2：recheck 只对 RootHost 有意义；裸 ServiceHost 直接回根状态 ok（宿主不回退）。
            Call::Recheck(_) => Ok(Response::Recheck(RecheckResponse {
                root: RootState::Ok,
                host: HostLocation::default(),
            })),
            // T008.3：设置接口由 RootHost 提供；裸宿主没有 Keyring。
            Call::ModelSettings(_)
            | Call::ConnectionSave(_)
            | Call::ConnectionDelete(_)
            | Call::ConnectionVerify(_)
            | Call::AvailableModels(_)
            | Call::ConnectionUpdate(_)
            | Call::ConnectionRevoke(_)
            | Call::KeyringRelocate(_)
            | Call::KeyringReopen(_)
            | Call::KeyringRestart(_)
            | Call::Issues(_)
            | Call::FirstUse(_)
            | Call::SessionModelSelect(_)
            | Call::ConnectionQuota(_) => Err(ServiceError::SettingsOnBareHost),
        }
    }

    /// 发消息：requestId 幂等（DSH prompt 的 requestId；判定在 Agent 的 durable rpcId 上：
    /// 同一 id 已进 inbox 或已落日志就不再插入，重启后仍然有效，T006.16b）。
    pub async fn send(&self, request: SendRequest) -> Result<SendResponse, ServiceError> {
        // 串行受理：并发同 id 时第一个进，其余在 Agent 的 durable 判重里变成 Duplicate。
        let _serialized = self.send_lock.lock().await;
        let admission = self
            .agent
            .send_with_request_id(
                &request.request_id,
                UserMessage::text(request.content),
                request.delivery.into(),
            )
            .await?;
        Ok(SendResponse {
            request_id: request.request_id,
            accepted: true,
            duplicate: matches!(admission, SendAdmission::Duplicate),
        })
    }

    /// 取消当前轮（清空 pending inbox + 中止正在跑的轮，照 T006.2a）。
    pub async fn cancel(&self, _request: CancelRequest) -> Result<CancelResponse, ServiceError> {
        self.agent.cancel().await?;
        Ok(CancelResponse { cancelled: true })
    }

    /// 回答整批：逐题走 T006.13 的 Agent::answer；有一题被拒就报错（已应用的保留）。
    pub async fn answer(&self, request: AnswerRequest) -> Result<AnswerResponse, ServiceError> {
        let mut applied = 0;
        for answer in request.answers {
            let accepted = self
                .agent
                .answer(&answer.question_id, answer.selected, answer.custom)
                .await;
            if !accepted {
                return Err(ServiceError::AnswerRejected(answer.question_id));
            }
            applied += 1;
        }
        Ok(AnswerResponse { applied })
    }

    /// 关掉不答（T006.16c）：撤掉当前全部待答问题（面板关掉的动作）。
    /// 逐条映射 T006.13 的 Agent::cancel_question：等待中的批整批标记撤销，整批 settle 后
    /// hook 以 ASK_ABORTED 工具错误收尾、本轮继续；重启恢复的批第一条就把整批移除，
    /// 已收的答案补写 question/answered、未答的写 question/cancelled（不投迟到回答）。
    pub async fn dismiss_questions(
        &self,
        _request: DismissQuestionsRequest,
    ) -> Result<DismissQuestionsResponse, ServiceError> {
        let mut dismissed = false;
        for question in self.agent.pending_questions() {
            if self.agent.cancel_question(&question.id).await {
                dismissed = true;
            }
        }
        Ok(DismissQuestionsResponse { dismissed })
    }

    /// 手动压缩（DSH command-compact）：返回替换计数，没有可压缩历史时 None。
    pub async fn compact(&self, request: CompactRequest) -> Result<CompactResponse, ServiceError> {
        let outcome = self.agent.compact(request.command_id).await?;
        Ok(CompactResponse {
            compacted: outcome.map(|outcome| CompactCounts {
                shadowed_items: outcome.shadowed_items,
                shadowed_tokens: outcome.shadowed_tokens,
            }),
        })
    }

    /// 被动状态第一版：对象 + 是否有一轮在跑 + 待答问题数。
    pub async fn status(&self, _request: StatusRequest) -> Result<StatusResponse, ServiceError> {
        Ok(StatusResponse {
            object: self.object(),
            running: self.agent.is_running(),
            pending_questions: self.agent.pending_questions().len(),
            stats: None,
            fx: None,
            currency: None,
        })
    }

    /// T006.18：停服务收尾——取消正在跑的一轮、等驱动空闲（日志收尾完整、写锁随 Agent drop 释放）。
    pub async fn shutdown(&self) {
        let _ = self.agent.cancel().await;
        self.agent.idle().await;
    }

    /// follow（DSH :607）：按调用方最后的序号补齐日志事件（连续、不重复；重连带 lastSeq 即可）。
    pub async fn follow(&self, request: FollowRequest) -> Result<FollowResponse, ServiceError> {
        let events = self.agent.session_events_from(request.last_seq);
        let last_seq = events
            .last()
            .map(|event| event.seq)
            .unwrap_or(request.last_seq);
        Ok(FollowResponse {
            events: events.iter().map(service_event).collect(),
            last_seq,
        })
    }

    /// events（DSH :540）：往回翻一页日志。默认预算 50/2/500（minMessages/minTurns/maxMessages：
    /// 从页尾往回数，走到**轮起点**且两条下限都满足时停；预算或日志耗尽先到就按先到的停）。
    pub async fn events(&self, request: EventsRequest) -> Result<EventsResponse, ServiceError> {
        let all = self.agent.session_events_from(0);
        let (selected, has_more) = page_events(
            &all,
            request.before_seq,
            request.max_messages.unwrap_or(500).max(1),
            request.min_messages,
            request.min_turns,
        );
        Ok(EventsResponse {
            events: selected.iter().map(|event| service_event(event)).collect(),
            has_more,
        })
    }
}

impl Drop for ServiceHost {
    fn drop(&mut self) {
        if let Some(handle) = self
            .forward
            .lock()
            .expect("service forward 锁被毒化")
            .take()
        {
            handle.abort();
        }
    }
}

/// T009.4：events 的分页选择（beforeSeq 不含本条；照 DSH history.ts:124-140）。
fn page_events(
    all: &[session::Event],
    before_seq: Option<u64>,
    max_messages: usize,
    min_messages: Option<usize>,
    min_turns: Option<usize>,
) -> (Vec<&session::Event>, bool) {
    if all.is_empty() {
        return (Vec::new(), false);
    }
    // T009.4：beforeSeq 是「不含本条」的上界（照 DSH history.ts:124-140）；省略 = 日志末端之后。
    let exclusive_end = match before_seq {
        Some(seq) => seq,
        None => all.last().map(|event| event.seq + 1).unwrap_or(0),
    };
    if exclusive_end == 0 {
        return (Vec::new(), false);
    }
    let min_messages = min_messages.unwrap_or(50).min(max_messages);
    let min_turns = min_turns.unwrap_or(2).max(1);
    let mut selected: Vec<&session::Event> = Vec::new();
    let mut messages = 0_usize;
    let mut turns = 0_usize;
    for event in all.iter().filter(|event| event.seq < exclusive_end).rev() {
        selected.push(event);
        match &event.kind {
            session::EventKind::UserMessage { .. }
            | session::EventKind::AssistantMessage { .. } => messages += 1,
            session::EventKind::TurnStart { .. } => turns += 1,
            _ => {}
        }
        if messages >= max_messages {
            break;
        }
        if matches!(event.kind, session::EventKind::TurnStart { .. })
            && messages >= min_messages
            && turns >= min_turns
        {
            break;
        }
    }
    selected.reverse();
    let first_seq = selected.first().map(|event| event.seq).unwrap_or(0);
    let has_more = first_seq > all[0].seq;
    (selected, has_more)
}

/// 会话日志事件 → 服务事件（data = 日志事件的字段，不含 type 标签）。
fn service_event(event: &session::Event) -> ServiceEvent {
    let mut data = serde_json::to_value(&event.kind).unwrap_or(serde_json::Value::Null);
    if let Some(object) = data.as_object_mut() {
        object.remove("type");
    }
    ServiceEvent {
        seq: event.seq,
        time: event.time,
        event_type: event.kind.type_name().to_owned(),
        data,
        cause: failure_cause(&event.kind),
    }
}

/// T007.2：失败原因分类只从日志里的 turn/end error code 派生（页面不按 code 猜）。
fn failure_cause(kind: &session::EventKind) -> Option<FailureCause> {
    let session::EventKind::TurnEnd {
        reason: session::TurnEndReason::Error { error },
        ..
    } = kind
    else {
        return None;
    };
    Some(match error.code.as_str() {
        "KEYRING" => FailureCause::Keyring,
        // T009.1：QUOTA 归凭证类（下一步是检查账户）。
        "AUTH" | "MISSING_CREDENTIAL" | "QUOTA" => FailureCause::Credential,
        _ => FailureCause::Model,
    })
}

/// 从一条服务事件里读上下文占用（assistant/message 的 usage 的 prompt 部分）。
fn context_used(event: &ServiceEvent) -> Option<u64> {
    if event.event_type != "assistant/message" {
        return None;
    }
    let usage = event.data.get("usage")?;
    let field = |name: &str| {
        usage
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0)
    };
    Some(field("inputTokens") + field("cacheReadTokens") + field("cacheWriteTokens"))
}

/// T008.F6：把设置页路径里的 ~ 展开为 home；非绝对路径/未知 ~ 用户明确拒绝。
fn expand_user_path(raw: &str, home: Option<&Path>) -> Result<PathBuf, ServiceError> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ServiceError::InvalidPath("路径不能为空".to_owned()));
    }
    let expanded = if raw == "~" || raw.starts_with("~/") {
        let home = home.ok_or_else(|| {
            ServiceError::InvalidPath("要找 home 目录，但当前进程没有 HOME".to_owned())
        })?;
        if raw == "~" {
            home.to_path_buf()
        } else {
            home.join(&raw[2..])
        }
    } else if raw.starts_with('~') {
        return Err(ServiceError::InvalidPath(
            "只支持 ~ 或 ~/ 开头；~user 不支持".to_owned(),
        ));
    } else {
        PathBuf::from(raw)
    };
    if !expanded.is_absolute() {
        return Err(ServiceError::InvalidPath(
            "要写完整路径（/ 或 ~/ 开头）".to_owned(),
        ));
    }
    Ok(expanded)
}

/// 根会话宿主（T007.2）：根会话可用时转发一个 ServiceHost；不可用时服务照常起，
/// ready 报状态、方法一律拒绝，recheck 修好后接管。协议方法只增不改。
pub struct RootHost {
    inner: tokio::sync::Mutex<Option<ServiceHost>>,
    state: Mutex<RootState>,
    session_id: Mutex<Option<String>>,
    context: Arc<Mutex<ContextUsage>>,
    options: RootSessionOptions,
    window: Mutex<u64>,
    events: broadcast::Sender<ServiceEvent>,
    agent_events: broadcast::Sender<agent::AgentEvent>,
    forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    agent_forward: Mutex<Option<tokio::task::JoinHandle<()>>>,
    /// T012.7：当前生效的模型选择（连接/模型/想要的档）；session/model 的 durable 事实
    /// 在会话日志里，这里只是进程内缓存（重开从日志恢复）。
    selection: Mutex<Option<session::ModelSelection>>,
}

impl RootHost {
    /// 打开根会话；打不开也返回宿主（状态进 ready，方法拒绝），由页面显示说明页。
    ///
    /// T008.3b：配置了 model_factory 时先解析当前 Keyring 连接；解析失败（空库/读不出）
    /// 仍返回宿主，状态报 Other，设置/首次使用方法照常可读。
    pub async fn open(mut options: RootSessionOptions, fallback_window: u64) -> Self {
        let (events, _) = broadcast::channel(SERVICE_EVENT_CHANNEL_CAPACITY);
        let (agent_events, _) = broadcast::channel(SERVICE_EVENT_CHANNEL_CAPACITY);
        let mut factory_error = None;
        let mut selection = None;
        if let Some(factory) = options.model_factory.clone() {
            // T012.7：重开会话从日志恢复选择；新会话由实现方按 settings/默认连接解析。
            let wanted = session_model_selection(&options.data_dir);
            match factory(wanted.as_ref()) {
                Ok((connection, model, credentials)) => {
                    selection = Some(session::ModelSelection {
                        connection,
                        model: model.model.clone(),
                        effort_wanted: model.effort.as_str().to_owned(),
                    });
                    options.model = model;
                    options.credentials = credentials;
                }
                Err(message) => factory_error = Some(message),
            }
        }
        let window = if options.model_factory.is_some() {
            options.model.context_window
        } else {
            fallback_window
        };
        let host = Self {
            inner: tokio::sync::Mutex::new(None),
            state: Mutex::new(RootState::Ok),
            session_id: Mutex::new(None),
            context: Arc::new(Mutex::new(ContextUsage { used: 0, window })),
            options,
            window: Mutex::new(window),
            events,
            agent_events,
            forward: Mutex::new(None),
            agent_forward: Mutex::new(None),
            selection: Mutex::new(selection),
        };
        if let Some(message) = factory_error {
            *host.state.lock().expect("root state 锁被毒化") = RootState::Unavailable {
                kind: RootUnavailableKind::Other,
                path: None,
                line: None,
                message,
            };
        } else {
            match open_root_session(host.options.clone()).await {
                Ok(root) => host.install(root.agent).await,
                Err(error) => {
                    *host.state.lock().expect("root state 锁被毒化") =
                        RootState::from_root_error(&error);
                }
            }
        }
        host
    }

    /// 接手一个已打开的 Agent：建 ServiceHost、把它的服务事件转发到本宿主的广播，
    /// 同时更新上下文占用（页面要的 context 只从日志事实算）。
    async fn install(&self, agent: Agent) {
        let session_id = agent.id().as_str().to_owned();
        // T007.4：流式文本等 AgentEvent 不落日志，单独转发给载体（只增帧类型）。
        let mut agent_source = agent.subscribe();
        let agent_tx = self.agent_events.clone();
        let agent_forward = tokio::spawn(async move {
            loop {
                match agent_source.recv().await {
                    Ok(event) => {
                        let _ = agent_tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        if let Some(handle) = self
            .agent_forward
            .lock()
            .expect("agent forward 锁被毒化")
            .replace(agent_forward)
        {
            handle.abort();
        }
        let service_host = ServiceHost::new(agent);
        let mut source = service_host.subscribe_events();
        let tx = self.events.clone();
        let context = Arc::clone(&self.context);
        let window = *self.window.lock().expect("window 锁被毒化");
        let forward = tokio::spawn(async move {
            loop {
                match source.recv().await {
                    Ok(event) => {
                        if let Some(used) = context_used(&event) {
                            *context.lock().expect("context 锁被毒化") =
                                ContextUsage { used, window };
                        }
                        let _ = tx.send(event);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        if let Some(handle) = self
            .forward
            .lock()
            .expect("forward 锁被毒化")
            .replace(forward)
        {
            handle.abort();
        }
        *self.inner.lock().await = Some(service_host);
        *self.state.lock().expect("root state 锁被毒化") = RootState::Ok;
        *self.session_id.lock().expect("session id 锁被毒化") = Some(session_id);
    }

    /// 宿主信息（ready/status 用）：协议版本 + 根状态 + 上下文占用。
    pub fn info(&self) -> HostInfo {
        let context = *self.context.lock().expect("context 锁被毒化");
        HostInfo {
            protocol_version: PROTOCOL_VERSION,
            session_id: self.session_id.lock().expect("session id 锁被毒化").clone(),
            root: self.state.lock().expect("root state 锁被毒化").clone(),
            context,
            host: HostLocation {
                home: std::env::var("HOME").ok().filter(|home| !home.is_empty()),
            },
        }
    }

    pub fn protocol_version(&self) -> u64 {
        PROTOCOL_VERSION
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }

    /// T007.4：AgentEvent 的 live 订阅（流式文本/用量等不落日志的观察面）。
    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<agent::AgentEvent> {
        self.agent_events.subscribe()
    }

    /// 再检查一次：修好之后接管根会话并返回新状态；还是不行就返回当前状态。
    pub async fn recheck(&self) -> RootState {
        if self.state.lock().expect("root state 锁被毒化").clone() == RootState::Ok {
            return RootState::Ok;
        }
        let mut options = self.options.clone();
        let wanted = self.selection.lock().expect("selection 锁被毒化").clone();
        if let Some(factory) = options.model_factory.clone() {
            match factory(wanted.as_ref()) {
                Ok((connection, model, credentials)) => {
                    *self.selection.lock().expect("selection 锁被毒化") =
                        Some(session::ModelSelection {
                            connection,
                            model: model.model.clone(),
                            effort_wanted: model.effort.as_str().to_owned(),
                        });
                    options.model = model;
                    options.credentials = credentials;
                }
                Err(message) => {
                    let state = RootState::Unavailable {
                        kind: RootUnavailableKind::Other,
                        path: None,
                        line: None,
                        message,
                    };
                    *self.state.lock().expect("root state 锁被毒化") = state.clone();
                    return state;
                }
            }
        }
        self.notify_keyring_changed();
        let window = options.model.context_window;
        *self.window.lock().expect("window 锁被毒化") = window;
        *self.context.lock().expect("context 锁被毒化") = ContextUsage { used: 0, window };
        match open_root_session(options).await {
            Ok(root) => {
                self.install(root.agent).await;
                RootState::Ok
            }
            Err(error) => {
                let state = RootState::from_root_error(&error);
                *self.state.lock().expect("root state 锁被毒化") = state.clone();
                state
            }
        }
    }

    /// T008.3：每次设置操作重新打开 Keyring（不缓存；换 key/换位置立即生效）。
    /// F1：Keyring 变化后把当前持有的 key 交给装配层的刷新钩子（监控脱敏用）。
    fn notify_keyring_changed(&self) {
        if let Some(hook) = &self.options.keyring_changed
            && let Ok(keyring) = self.keyring()
        {
            hook(&keyring);
        }
    }

    fn keyring(&self) -> Result<credentials::Keyring, ServiceError> {
        Ok(credentials::Keyring::open(
            &self.options.data_dir,
            self.options.identity.as_deref(),
        )?)
    }

    /// T008.3：设置页的 Keyring 视图（响应不含 key 正文）。
    pub async fn model_settings(&self) -> Result<ModelSettingsResponse, ServiceError> {
        Ok(ModelSettingsResponse {
            settings: self.keyring()?.view(),
        })
    }

    pub async fn connection_save(
        &self,
        request: ConnectionSaveRequest,
    ) -> Result<ConnectionSaveResponse, ServiceError> {
        let connection = self
            .keyring()?
            .save_connection(credentials::NewConnection {
                id: request.id,
                name: request.name,
                model: request.model,
                endpoint: request.endpoint,
                api_key: request.api_key,
                preset: request.preset,
                models: request.models,
                price_rate: request.price_rate,
                auth_header: request.auth_header,
            })?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(ConnectionSaveResponse { connection })
    }

    pub async fn connection_delete(
        &self,
        request: ConnectionDeleteRequest,
    ) -> Result<ConnectionDeleteResponse, ServiceError> {
        let deleted = self.keyring()?.delete_connection(&request.id)?;
        if deleted {
            self.emit_settings_changed();
            self.notify_keyring_changed();
        }
        Ok(ConnectionDeleteResponse { deleted })
    }

    /// T008.3 增补：编辑连接（只改 endpoint/model，保留 key）。
    pub async fn connection_update(
        &self,
        request: ConnectionUpdateRequest,
    ) -> Result<ConnectionUpdateResponse, ServiceError> {
        let connection = self.keyring()?.update_connection_details(
            &request.id,
            credentials::ConnectionUpdate {
                endpoint: request.endpoint,
                model: request.model,
                preset: request.preset,
                models: request.models,
                price_rate: request.price_rate,
                auth_header: request.auth_header,
            },
        )?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(ConnectionUpdateResponse { connection })
    }

    /// T008.3 增补：撤销 key（连接保留）。
    pub async fn connection_revoke(
        &self,
        request: ConnectionRevokeRequest,
    ) -> Result<ConnectionRevokeResponse, ServiceError> {
        let connection = self.keyring()?.revoke_key(&request.id)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(ConnectionRevokeResponse { connection })
    }

    /// T008.3：验证连接：从 Keyring 取一次签发，发最小 Messages 请求，把结果写进注册器。
    pub async fn connection_verify(
        &self,
        request: ConnectionVerifyRequest,
    ) -> Result<ConnectionVerifyResponse, ServiceError> {
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        let api_key = keyring
            .issue(&request.id, "connection-verify", None, None)?
            .into_secret();
        // T009.3：验证走模型适配器（和对话同一条请求路径/同一组头），不再手写 reqwest。
        let config = model::DeepSeekConfig {
            base_url: connection.endpoint.clone(),
            // K3：验证用连接所选模型（新形状的 models 第一行）序列化。
            model: connection.primary_model().to_owned(),
            max_tokens: 1,
            context_window: model::DEFAULT_CONTEXT_WINDOW,
            effort: model::Effort::new("off"),
        };
        let cancel = model::CancelToken::new(
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            std::sync::Arc::new(tokio::sync::Notify::new()),
        );
        let messages = vec![model::Message::user_text("ping")];
        let at = now_seconds();
        // T012.6：验证也必须能把学到的鉴权头写回连接，所以把 Keyring 的 Arc 交给来源。
        let keyring = std::sync::Arc::new(keyring);
        let result = match model::DeepSeek::with_credentials(
            config,
            std::sync::Arc::new(VerifyCredential {
                api_key,
                keyring: std::sync::Arc::clone(&keyring),
                connection: request.id.clone(),
            }),
        ) {
            Err(error) => credentials::CheckResult::Fail {
                at,
                reason: error.code.as_str().to_owned(),
            },
            Ok(model) => match model.stream(&messages, &[], &cancel).await {
                Ok(_) => credentials::CheckResult::Ok { at },
                Err(error) => credentials::CheckResult::Fail {
                    at,
                    reason: error.code.as_str().to_owned(),
                },
            },
        };
        keyring.record_check(&request.id, result)?;
        self.emit_settings_changed();
        let connection = keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        Ok(ConnectionVerifyResponse { connection })
    }

    /// T012.5：目录 + 厂商 GET /v1/models 合并；用户填的字段最后覆盖。
    pub async fn available_models(
        &self,
        request: AvailableModelsRequest,
    ) -> Result<AvailableModelsResponse, ServiceError> {
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        let key = keyring.issue(&request.id, "models-list", None, None)?;
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let provider_id = if connection.preset.is_empty() || connection.preset == "custom" {
            model::catalog::provider_id_for_endpoint(&connection.endpoint)
        } else {
            connection.preset.clone()
        };
        let mut models = catalog.models_for(&provider_id);
        let mut live = false;
        if let Ok(client) = model::http::client() {
            // 审计 T012.5 fix-now 1：endpoint 以 /anthropic 结尾时列表在 Chat base 的 /v1/models。
            let url = model::catalog::models_url(&connection.endpoint);
            let mut request = client.get(url).timeout(std::time::Duration::from_secs(5));
            match connection.auth_header.as_deref() {
                Some("authorization") => {
                    request =
                        request.header("authorization", format!("Bearer {}", key.into_secret()));
                }
                _ => {
                    request = request.header("x-api-key", key.into_secret());
                }
            }
            if let Ok(response) = request.send().await
                && response.status().is_success()
                && let Ok(bytes) = response.bytes().await
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes)
            {
                let live_models = model::catalog::parse_live_models(&value);
                if !live_models.is_empty() {
                    models = model::catalog::merge_live(models, live_models);
                    live = true;
                }
            }
        }
        // 用户填过的字段不动：同名覆盖 name/窗口/输出/用户价格；目录没有的也列出来。
        for row in &connection.models {
            if let Some(info) = models.iter_mut().find(|info| info.id == row.id) {
                if let Some(name) = &row.name {
                    info.name = name.clone();
                }
                if row.context_window.is_some() {
                    info.context_window = row.context_window;
                }
                if row.max_tokens.is_some() {
                    info.max_tokens = row.max_tokens;
                }
                if let Some(price) = row.price {
                    info.price_input = Some(price);
                    info.price_output = Some(price);
                }
            } else {
                models.push(model::catalog::ModelInfo {
                    id: row.id.clone(),
                    name: row.name.clone().unwrap_or_else(|| row.id.clone()),
                    context_window: row.context_window,
                    max_tokens: row.max_tokens,
                    price_input: row.price,
                    price_output: row.price,
                    thinks: false,
                    efforts: Vec::new(),
                });
            }
        }
        Ok(AvailableModelsResponse { models, live })
    }

    pub async fn keyring_relocate(
        &self,
        request: KeyringRelocateRequest,
    ) -> Result<KeyringRelocateResponse, ServiceError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = expand_user_path(&request.to, home.as_deref())?;
        self.keyring()?.relocate_identity(&path)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(KeyringRelocateResponse {})
    }

    pub async fn keyring_reopen(
        &self,
        request: KeyringReopenRequest,
    ) -> Result<KeyringReopenResponse, ServiceError> {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let path = expand_user_path(&request.at, home.as_deref())?;
        self.keyring()?.repoint_identity(&path)?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(KeyringReopenResponse {})
    }

    pub async fn keyring_restart(&self) -> Result<KeyringRestartResponse, ServiceError> {
        let keyring = self.keyring()?;
        keyring.restart()?;
        self.emit_settings_changed();
        self.notify_keyring_changed();
        Ok(KeyringRestartResponse {
            settings: keyring.view(),
        })
    }

    pub async fn issues(&self) -> Result<IssuesResponse, ServiceError> {
        let view = self.keyring()?.view();
        Ok(IssuesResponse {
            issued_today: view.issued_today,
            issues: view.issues,
        })
    }

    pub async fn first_use(&self) -> Result<FirstUseResponse, ServiceError> {
        let view = self.keyring()?.view();
        Ok(FirstUseResponse {
            first_use: view.first_use,
            key_file: view.key_file,
        })
    }

    /// T012.8b：查询连接的余额/套餐余量（DeepSeek /user/balance、MiniMax /v1/token_plan/remains）。
    /// 查不到不报错（字段为空；spec 故事 5）。
    pub async fn connection_quota(
        &self,
        request: ConnectionQuotaRequest,
    ) -> Result<ConnectionQuotaResponse, ServiceError> {
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .into_iter()
            .find(|connection| connection.id == request.id)
            .ok_or_else(|| {
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    request.id.clone(),
                ))
            })?;
        let key = keyring
            .issue(&request.id, "connection-quota", None, None)?
            .into_secret();
        let quota = quota::fetch(&connection, &key).await;
        Ok(ConnectionQuotaResponse {
            balance: quota.balance,
            plan: quota.plan,
            checked_at: now_seconds(),
        })
    }

    /// T012.8：状态（对象/在跑/待答问题）+ 会话统计（轮/步/token/缓存/上下文/花费）+ 汇率 + 币种。
    ///
    /// 统计和花费从日志现算（不落日志）：每轮模型取 assistant/message 的 message.source.model，
    /// 价格按当前连接 + 目录（用户价 > 标价×倍率 > 无价格；无价格的轮标 unpriced）。
    async fn host_status(&self, _request: StatusRequest) -> Result<StatusResponse, ServiceError> {
        let (object, running, pending_questions, agent) = {
            let guard = self.inner.lock().await;
            let Some(host) = guard.as_ref() else {
                return Err(ServiceError::RootUnavailable(
                    self.state.lock().expect("root state 锁被毒化").clone(),
                ));
            };
            (
                host.object(),
                host.agent().is_running(),
                host.agent().pending_questions().len(),
                host.agent().clone(),
            )
        };
        let events = agent.session_events_from(0);
        let current = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .unwrap_or_else(|| session::ModelSelection {
                connection: "deepseek".to_owned(),
                model: self.options.model.model.clone(),
                effort_wanted: self.options.model.effort.as_str().to_owned(),
            });
        let settings = settings::load(&self.options.data_dir);
        let keyring = self.keyring()?;
        let view = keyring.view();
        let connection = view
            .connections
            .iter()
            .find(|connection| connection.id == current.connection)
            .or_else(|| view.connections.first())
            .cloned();
        let catalog = model::catalog::global().unwrap_or_else(model::catalog::snapshot);
        let (context_used, context_window) = {
            let context = self.context.lock().expect("context 锁被毒化");
            (context.used, context.window)
        };
        let stats = match connection {
            Some(connection) => stats::session_stats(
                &events,
                &connection,
                &catalog,
                &current,
                context_used,
                context_window,
            ),
            None => stats::SessionStats {
                context_used,
                context_window,
                connection: current.connection.clone(),
                model: current.model.clone(),
                effort_wanted: current.effort_wanted.clone(),
                ..stats::SessionStats::default()
            },
        };
        let fx = fx::usd_to_cny(&self.options.data_dir).await;
        Ok(StatusResponse {
            object,
            running,
            pending_questions,
            stats: Some(stats),
            fx: Some(fx),
            currency: Some(settings.currency),
        })
    }

    /// T008.3/T012.7：选模型（连接/模型/想要的档；下一轮生效，D26）。
    ///
    /// 选择是 durable 事实：写 session/model 事件（重开会话从日志恢复）并更新 settings.json
    /// 的上次选择（新会话继承）。同连接换模型/档只改下一轮；跨连接时若当前空闲则重建根
    /// agent（新连接从下一轮开始用）；正在跑一轮时只记录选择，重建留到 recheck/重启
    /// （known：运行中跨连接热切换不打断当前轮）。
    pub async fn session_model_select(
        &self,
        request: SessionModelSelectRequest,
    ) -> Result<SessionModelSelectResponse, ServiceError> {
        let current = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .unwrap_or_else(|| session::ModelSelection {
                connection: "deepseek".to_owned(),
                model: self.options.model.model.clone(),
                effort_wanted: self.options.model.effort.as_str().to_owned(),
            });
        let target = session::ModelSelection {
            connection: request
                .connection
                .unwrap_or_else(|| current.connection.clone()),
            model: request.model.unwrap_or_else(|| current.model.clone()),
            effort_wanted: request
                .effort_wanted
                .unwrap_or_else(|| current.effort_wanted.clone()),
        };
        // settings.json：新会话默认用上一次选的（story 2）。
        let mut settings = settings::load(&self.options.data_dir);
        settings.last_selection = Some(target.clone());
        let _ = settings::save(&self.options.data_dir, &settings);
        let cross_connection = target.connection != current.connection;
        let idle = {
            let guard = self.inner.lock().await;
            let Some(host) = guard.as_ref() else {
                return Err(ServiceError::RootUnavailable(
                    self.state.lock().expect("root state 锁被毒化").clone(),
                ));
            };
            host.agent().select(&target).await?;
            !host.agent().is_running()
        };
        *self.selection.lock().expect("selection 锁被毒化") = Some(target.clone());
        if cross_connection && idle {
            self.rebuild_with(&target).await?;
        }
        // T012.7 审计 P2：响应报实际生效的选择（跨连接重建后 factory 解析的 connection 可能不同）。
        let actual = self
            .selection
            .lock()
            .expect("selection 锁被毒化")
            .clone()
            .unwrap_or(target);
        Ok(SessionModelSelectResponse {
            connection: actual.connection,
            model: actual.model,
            effort_wanted: actual.effort_wanted,
        })
    }

    /// T012.7：按新的选择重建根 agent（跨连接热切换；用 factory 解析新连接）。
    async fn rebuild_with(&self, selection: &session::ModelSelection) -> Result<(), ServiceError> {
        let mut options = self.options.clone();
        if let Some(factory) = options.model_factory.clone() {
            match factory(Some(selection)) {
                Ok((connection, model, credentials)) => {
                    *self.selection.lock().expect("selection 锁被毒化") =
                        Some(session::ModelSelection {
                            connection,
                            model: model.model.clone(),
                            effort_wanted: model.effort.as_str().to_owned(),
                        });
                    options.model = model;
                    options.credentials = credentials;
                }
                Err(message) => {
                    let state = RootState::Unavailable {
                        kind: RootUnavailableKind::Other,
                        path: None,
                        line: None,
                        message,
                    };
                    *self.state.lock().expect("root state 锁被毒化") = state.clone();
                    return Err(ServiceError::RootUnavailable(state));
                }
            }
        }
        // 先放掉旧 Agent（释放会话写锁）再重开同一会话。
        let old = self.inner.lock().await.take();
        drop(old);
        match open_root_session(options).await {
            Ok(root) => {
                self.install(root.agent).await;
                Ok(())
            }
            Err(error) => {
                let state = RootState::from_root_error(&error);
                *self.state.lock().expect("root state 锁被毒化") = state.clone();
                Err(ServiceError::RootUnavailable(state))
            }
        }
    }

    /// 唯一调用入口；根会话不可用时除 recheck 外一律拒绝。
    pub async fn dispatch(&self, call: Call) -> Result<Response, ServiceError> {
        if matches!(call, Call::Recheck(_)) {
            let root = self.recheck().await;
            let host = self.info().host;
            return Ok(Response::Recheck(RecheckResponse { root, host }));
        }
        match call {
            Call::ModelSettings(_) => self.model_settings().await.map(Response::ModelSettings),
            Call::ConnectionSave(request) => self
                .connection_save(request)
                .await
                .map(Response::ConnectionSave),
            Call::ConnectionDelete(request) => self
                .connection_delete(request)
                .await
                .map(Response::ConnectionDelete),
            Call::ConnectionVerify(request) => self
                .connection_verify(request)
                .await
                .map(Response::ConnectionVerify),
            Call::AvailableModels(request) => self
                .available_models(request)
                .await
                .map(Response::AvailableModels),
            Call::ConnectionUpdate(request) => self
                .connection_update(request)
                .await
                .map(Response::ConnectionUpdate),
            Call::ConnectionRevoke(request) => self
                .connection_revoke(request)
                .await
                .map(Response::ConnectionRevoke),
            Call::KeyringRelocate(request) => self
                .keyring_relocate(request)
                .await
                .map(Response::KeyringRelocate),
            Call::KeyringReopen(request) => self
                .keyring_reopen(request)
                .await
                .map(Response::KeyringReopen),
            Call::KeyringRestart(_) => self.keyring_restart().await.map(Response::KeyringRestart),
            Call::Issues(_) => self.issues().await.map(Response::Issues),
            Call::FirstUse(_) => self.first_use().await.map(Response::FirstUse),
            Call::SessionModelSelect(request) => self
                .session_model_select(request)
                .await
                .map(Response::SessionModelSelect),
            // T012.8：根宿主自己填统计/汇率/币种（需要 data_dir + Keyring）。
            Call::Status(request) => self.host_status(request).await.map(Response::Status),
            // T012.8b：余额/余量需要 Keyring 签发 key。
            Call::ConnectionQuota(request) => self
                .connection_quota(request)
                .await
                .map(Response::ConnectionQuota),
            other => {
                let guard = self.inner.lock().await;
                match guard.as_ref() {
                    Some(host) => host.dispatch(other).await,
                    None => Err(ServiceError::RootUnavailable(
                        self.state.lock().expect("root state 锁被毒化").clone(),
                    )),
                }
            }
        }
    }

    pub async fn follow(&self, request: FollowRequest) -> Result<FollowResponse, ServiceError> {
        let guard = self.inner.lock().await;
        match guard.as_ref() {
            Some(host) => host.follow(request).await,
            None => Err(ServiceError::RootUnavailable(
                self.state.lock().expect("root state 锁被毒化").clone(),
            )),
        }
    }

    pub async fn shutdown(&self) {
        let guard = self.inner.lock().await;
        if let Some(host) = guard.as_ref() {
            host.shutdown().await;
        }
    }
}

impl RootHost {
    /// T008.3：设置变化通知（seq=0 的合成事件；页面收到后重新拉设置）。
    fn emit_settings_changed(&self) {
        let event = ServiceEvent {
            seq: 0,
            time: 0,
            event_type: "keyring/changed".to_owned(),
            data: serde_json::json!({}),
            cause: None,
        };
        // 没有订阅端是正常情况（服务刚起/页面没开）；不把广播失败当错误。
        let _delivered = self.events.send(event).unwrap_or(0);
    }
}

/// T009.3：验证连接的固定 key 来源（key 只在内存里；签发记录仍由 Keyring 记）。
/// T012.6：验证时学到的鉴权头也写回连接（和对话路径同一接缝）。
struct VerifyCredential {
    api_key: String,
    keyring: std::sync::Arc<credentials::Keyring>,
    connection: String,
}

impl model::CredentialSource for VerifyCredential {
    fn issue(
        &self,
        _purpose: &str,
        _context: &model::CredentialContext,
    ) -> Result<String, model::ModelError> {
        Ok(self.api_key.clone())
    }

    /// T012.6 P2：连接上记住的鉴权头（第二次验证直接用它，不再从双头开始）。
    fn auth_header(&self) -> Option<String> {
        self.keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == self.connection)
            .and_then(|connection| connection.auth_header)
    }

    fn remember_auth_header(&self, header: &str) {
        let Some(connection) = self
            .keyring
            .view()
            .connections
            .into_iter()
            .find(|connection| connection.id == self.connection)
        else {
            return;
        };
        let _ = self.keyring.update_connection_details(
            &self.connection,
            credentials::ConnectionUpdate {
                endpoint: connection.endpoint.clone(),
                model: connection.primary_model().to_owned(),
                auth_header: Some(header.to_owned()),
                ..credentials::ConnectionUpdate::default()
            },
        );
    }
}

fn now_seconds() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

impl Drop for RootHost {
    fn drop(&mut self) {
        if let Some(handle) = self.forward.lock().expect("forward 锁被毒化").take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
mod t007_protocol_tests {
    use super::*;

    fn turn_end(code: &str) -> session::EventKind {
        session::EventKind::TurnEnd {
            turn: 1,
            reason: session::TurnEndReason::Error {
                error: session::LlmFailure {
                    message: "boom".to_owned(),
                    code: code.to_owned(),
                },
            },
        }
    }

    #[test]
    fn failure_cause_classifies_credential_and_model() {
        assert_eq!(
            failure_cause(&turn_end("AUTH")),
            Some(FailureCause::Credential)
        );
        assert_eq!(
            failure_cause(&turn_end("MISSING_CREDENTIAL")),
            Some(FailureCause::Credential)
        );
        assert_eq!(
            failure_cause(&turn_end("SERVER")),
            Some(FailureCause::Model)
        );
        let keyring = session::EventKind::TurnEnd {
            turn: 1,
            reason: session::TurnEndReason::Error {
                error: session::LlmFailure {
                    message: "Keyring 无法签发：钥匙文件不在".to_owned(),
                    code: "KEYRING".to_owned(),
                },
            },
        };
        assert_eq!(failure_cause(&keyring), Some(FailureCause::Keyring));
        assert_eq!(
            failure_cause(&session::EventKind::StepStart { turn: 1, step: 1 }),
            None
        );
    }

    #[test]
    fn context_used_reads_prompt_tokens_from_usage() {
        let event = ServiceEvent {
            seq: 1,
            time: 1,
            event_type: "assistant/message".to_owned(),
            data: serde_json::json!({"usage": {"inputTokens": 100, "cacheReadTokens": 20, "cacheWriteTokens": 5, "outputTokens": 9}}),
            cause: None,
        };
        assert_eq!(context_used(&event), Some(125));
        let non_message = ServiceEvent {
            seq: 2,
            time: 2,
            event_type: "turn/start".to_owned(),
            data: serde_json::json!({}),
            cause: None,
        };
        assert_eq!(context_used(&non_message), None);
    }
}

#[cfg(test)]
mod f5_failure_cause_tests {
    use super::*;

    fn turn_end(code: &str, message: &str) -> session::EventKind {
        session::EventKind::TurnEnd {
            turn: 1,
            reason: session::TurnEndReason::Error {
                error: session::LlmFailure {
                    message: message.to_owned(),
                    code: code.to_owned(),
                },
            },
        }
    }

    #[test]
    fn failure_cause_keyring_uses_structured_code() {
        assert_eq!(
            failure_cause(&turn_end("KEYRING", "任何消息")),
            Some(FailureCause::Keyring),
            "KEYRING 码必须映射 Keyring，不能看消息文本"
        );
        assert_eq!(
            failure_cause(&turn_end(
                "MISSING_CREDENTIAL",
                "Keyring 无法签发：钥匙文件不在"
            )),
            Some(FailureCause::Credential),
            "消息里有 Keyring 也不能改变码的分类"
        );
    }
}

#[cfg(test)]
mod f6_path_tests {
    use super::*;

    #[test]
    fn tilde_paths_expand_and_relative_is_rejected() {
        let home = Path::new("/home/user");
        assert_eq!(
            expand_user_path("~/Backups/keyring.key", Some(home)).expect("tilde"),
            PathBuf::from("/home/user/Backups/keyring.key")
        );
        assert_eq!(
            expand_user_path("/tmp/keyring.key", None).expect("absolute"),
            PathBuf::from("/tmp/keyring.key")
        );
        assert!(expand_user_path("relative/keyring.key", Some(home)).is_err());
        assert!(expand_user_path("~other/keyring.key", Some(home)).is_err());
    }
}

#[cfg(test)]
mod t009_error_tests {
    use super::*;

    fn variants() -> Vec<(&'static str, ServiceError)> {
        vec![
            (
                "agent/internal",
                ServiceError::Agent(agent::AgentError::Compaction("x".to_owned())),
            ),
            (
                "question/answer-rejected",
                ServiceError::AnswerRejected("q1".to_owned()),
            ),
            (
                "root/unavailable",
                ServiceError::RootUnavailable(RootState::Ok),
            ),
            (
                "keyring/key-file-missing",
                ServiceError::Credentials(credentials::KeyringError::KeyFileMissing(
                    PathBuf::from("/k"),
                )),
            ),
            (
                "keyring/undecryptable",
                ServiceError::Credentials(credentials::KeyringError::Crypto("x".to_owned())),
            ),
            (
                "keyring/corrupt",
                ServiceError::Credentials(credentials::KeyringError::Corrupt("x".to_owned())),
            ),
            (
                "keyring/io",
                ServiceError::Credentials(credentials::KeyringError::Io(std::io::Error::other(
                    "x",
                ))),
            ),
            (
                "connection/missing",
                ServiceError::Credentials(credentials::KeyringError::MissingConnection(
                    "deepseek".to_owned(),
                )),
            ),
            (
                "connection/invalid",
                ServiceError::Credentials(credentials::KeyringError::InvalidConnection(
                    "x".to_owned(),
                )),
            ),
            ("settings/unavailable", ServiceError::SettingsOnBareHost),
            (
                "request/invalid-path",
                ServiceError::InvalidPath("bad".to_owned()),
            ),
        ]
    }

    #[test]
    fn service_error_codes_cover_every_variant() {
        let pairs = variants();
        for (expected, error) in &pairs {
            assert_eq!(error.code().as_str(), *expected, "变体的码");
        }
        let mut used: Vec<&str> = pairs.iter().map(|(code, _)| *code).collect();
        used.sort_unstable();
        used.dedup();
        // spec v1.1：12 码 = ServiceError 的 11 个变体码 + web 层的 request/invalid-json。
        assert_eq!(ServiceErrorCode::ALL.len(), 12, "码数 12");
        assert!(
            ServiceErrorCode::ALL
                .iter()
                .any(|code| code.as_str() == "request/invalid-json"),
            "invalid-json 在唯一码表里"
        );
        let mut all: Vec<&str> = ServiceErrorCode::ALL
            .iter()
            .map(|code| code.as_str())
            .filter(|code| *code != "request/invalid-json")
            .collect();
        all.sort_unstable();
        assert_eq!(
            used, all,
            "码表与 ServiceError 变体必须一一对应（多/少都失败；invalid-json 由 web 层用）"
        );
    }

    #[test]
    fn invalid_json_code_is_in_the_single_code_table() {
        assert_eq!(ServiceErrorCode::ALL.len(), 12, "码数 12（spec v1.1）");
        assert!(
            ServiceErrorCode::ALL
                .iter()
                .any(|code| code.as_str() == "request/invalid-json"),
            "request/invalid-json 必须进唯一码表 ALL"
        );
    }

    #[test]
    fn page_error_table_covers_service_and_model_codes() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/src/data/errors.ts");
        let table = std::fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("页面错误表 {} 读不到：{error}", path.display()));
        for code in ServiceErrorCode::ALL {
            assert!(
                table.contains(code.as_str()),
                "页面对照表缺 {} 这一行",
                code.as_str()
            );
        }
        // 日志里的模型错误码（spec 测试清单 2）。
        for code in [
            "AUTH",
            "MISSING_CREDENTIAL",
            "KEYRING",
            "QUOTA",
            "RATE_LIMIT",
            "CONTEXT_WINDOW_EXCEEDED",
            "INVALID_REQUEST",
            "SERVER",
            "TRANSPORT",
            "TIMEOUT",
            "STREAM_CLOSED",
            "MALFORMED_RESPONSE",
            "EMPTY_RESPONSE",
            "UNSUPPORTED_CONTENT",
            "UNSUPPORTED_REASONING_EFFORT",
            "ABORTED",
        ] {
            assert!(table.contains(code), "页面对照表缺模型码 {code} 这一行");
        }
    }

    #[test]
    fn service_error_body_has_code_message_and_root_details() {
        let root_error = ServiceError::RootUnavailable(RootState::Unavailable {
            kind: RootUnavailableKind::Other,
            path: Some("/x".to_owned()),
            line: Some(3),
            message: "x".to_owned(),
        });
        let body = ServiceErrorBody::from_service_error(&root_error);
        let json = serde_json::to_value(&body).expect("serialize");
        assert_eq!(json["code"], "root/unavailable");
        assert!(
            json["message"]
                .as_str()
                .is_some_and(|message| !message.is_empty())
        );
        assert_eq!(json["details"]["protocolVersion"], PROTOCOL_VERSION);
        assert!(
            json["details"].get("root").is_some(),
            "details.root：{json}"
        );
        assert!(json.get("error").is_none(), "不再有旧 error 字段：{json}");
        let other =
            ServiceErrorBody::from_service_error(&ServiceError::AnswerRejected("q1".to_owned()));
        let other_json = serde_json::to_value(&other).expect("serialize");
        assert_eq!(other_json["code"], "question/answer-rejected");
        assert!(
            other_json.get("details").is_none(),
            "无 detail 时不出现该字段：{other_json}"
        );
        assert!(other_json.get("error").is_none());
    }

    #[test]
    fn quota_error_is_credential_cause() {
        assert_eq!(
            failure_cause(&turn_end("QUOTA", "额度用完")),
            Some(FailureCause::Credential),
            "QUOTA 归凭证类"
        );
    }

    fn turn_end(code: &str, message: &str) -> session::EventKind {
        session::EventKind::TurnEnd {
            turn: 1,
            reason: session::TurnEndReason::Error {
                error: session::LlmFailure {
                    message: message.to_owned(),
                    code: code.to_owned(),
                },
            },
        }
    }
}

#[cfg(test)]
mod t009_pagination_tests {
    use super::*;

    fn events(count: u64) -> Vec<session::Event> {
        (0..count)
            .map(|seq| {
                session::Event::new(
                    seq,
                    1_000 + seq,
                    session::EventKind::UserMessage {
                        turn: seq / 2 + 1,
                        step: 1,
                        message: model::Message::text(model::Role::User, "m"),
                    },
                )
            })
            .collect()
    }

    fn seqs(page: &[&session::Event]) -> Vec<u64> {
        page.iter().map(|event| event.seq).collect()
    }

    #[test]
    fn before_seq_is_exclusive_upper_bound() {
        let all = events(10);
        let (page, has_more) = page_events(&all, Some(5), 100, None, None);
        assert_eq!(seqs(&page), vec![0, 1, 2, 3, 4], "beforeSeq 不含本条");
        assert!(!has_more);
        let (page, _) = page_events(&all, Some(0), 100, None, None);
        assert!(page.is_empty(), "beforeSeq=0 没有任何更早的事件");
        let (page, _) = page_events(&all, Some(11), 100, None, None);
        assert_eq!(
            seqs(&page),
            (0..10).collect::<Vec<_>>(),
            "beforeSeq=N+1 等于全部"
        );
        let (page, _) = page_events(&all, None, 100, None, None);
        assert_eq!(
            seqs(&page),
            (0..10).collect::<Vec<_>>(),
            "省略 beforeSeq 等于全部"
        );
    }

    #[test]
    fn paging_backwards_yields_each_seq_exactly_once() {
        let all = events(10);
        for page_size in [2_usize, 3] {
            let mut cursor: Option<u64> = None;
            let mut collected: Vec<u64> = Vec::new();
            for _ in 0..100 {
                let (page, has_more) = page_events(&all, cursor, page_size, None, None);
                if page.is_empty() {
                    break;
                }
                collected.extend(seqs(&page));
                cursor = page.first().map(|event| event.seq);
                if !has_more {
                    break;
                }
            }
            collected.sort_unstable();
            assert_eq!(
                collected,
                (0..10).collect::<Vec<_>>(),
                "page_size={page_size}：每条正好一次、不缺"
            );
        }
    }

    #[test]
    fn protocol_version_is_4() {
        assert_eq!(PROTOCOL_VERSION, 4);
    }
}
