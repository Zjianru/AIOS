//! 线上协议形状：调用面 Call/Response、事件面 ServiceEvent 及其请求/响应类型。
//! 只放数据形状和序列化，不放行为。

use serde::{Deserialize, Serialize};

use crate::{fx, quota, stats};

/// 服务对象：固定 Kind 根会话（16b 换成根会话实体）；第一版用会话 id 表达。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Object {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// 投递方式（服务协议里用稳定的 kebab-case 名）。
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

/// 一次调用（载体把线上请求解码成它）。
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
    /// 页面没接住的异常 / 事件流重连（只记 web 监控 ops，不进会话日志）。
    MonitorReport(MonitorReportRequest),
    /// 再检查一次根会话（修好 missing/corrupt/busy 之后回到对话）。
    Recheck(RecheckRequest),
    ModelSettings(ModelSettingsRequest),
    /// 整表替换屏蔽的模型规则（立即保存、立即生效）。
    DisabledModelsSet(DisabledModelsSetRequest),
    /// 一条屏蔽规则现在匹配哪些模型、谁在用（确认框）。
    DisabledModelsCheck(DisabledModelsCheckRequest),
    ConnectionSave(ConnectionSaveRequest),
    ConnectionDelete(ConnectionDeleteRequest),
    /// 只改 endpoint/model，保留 key。
    ConnectionUpdate(ConnectionUpdateRequest),
    /// 撤销 key，连接保留。
    ConnectionRevoke(ConnectionRevokeRequest),
    /// 验证连接（最小请求）；结果写进注册器。
    ConnectionVerify(ConnectionVerifyRequest),
    /// 获取可用模型（目录 + 厂商列表）。
    AvailableModels(AvailableModelsRequest),
    KeyringRelocate(KeyringRelocateRequest),
    KeyringReopen(KeyringReopenRequest),
    KeyringRestart(KeyringRestartRequest),
    Issues(IssuesRequest),
    FirstUse(FirstUseRequest),
    /// 选模型（下一轮生效，当前轮不换）。
    SessionModelSelect(SessionModelSelectRequest),
    /// 查询连接余额/套餐余量。
    ConnectionQuota(ConnectionQuotaRequest),
    /// 写非密钥设置（currency；只写 settings.json）。
    SettingsUpdate(SettingsUpdateRequest),
    /// 存一份文字附件（只读副本），返回 id。
    AttachmentPut(AttachmentPutRequest),
    /// 按 id 读回附件（卡片「点开看全文」）。
    AttachmentGet(AttachmentGetRequest),
    /// @ 文件候选（按会话工作目录）。
    FileReferencesList(FileReferencesListRequest),
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
    MonitorReport(MonitorReportResponse),
    Recheck(RecheckResponse),
    ModelSettings(ModelSettingsResponse),
    DisabledModelsSet(DisabledModelsSetResponse),
    DisabledModelsCheck(DisabledModelsCheckResponse),
    ConnectionSave(ConnectionSaveResponse),
    ConnectionDelete(ConnectionDeleteResponse),
    ConnectionUpdate(ConnectionUpdateResponse),
    ConnectionRevoke(ConnectionRevokeResponse),
    ConnectionVerify(ConnectionVerifyResponse),
    /// 连接余额/套餐余量。
    ConnectionQuota(ConnectionQuotaResponse),
    /// 写非密钥设置。
    SettingsUpdate(SettingsUpdateResponse),
    AvailableModels(AvailableModelsResponse),
    KeyringRelocate(KeyringRelocateResponse),
    KeyringReopen(KeyringReopenResponse),
    KeyringRestart(KeyringRestartResponse),
    Issues(IssuesResponse),
    FirstUse(FirstUseResponse),
    SessionModelSelect(SessionModelSelectResponse),
    /// 附件仓库。
    AttachmentPut(AttachmentPutResponse),
    AttachmentGet(AttachmentGetResponse),
    /// @ 文件候选。
    FileReferencesList(FileReferencesListResponse),
}

/// 发消息（DSH session-controller 的 prompt）。requestId 是幂等键：同一 id 只进一次。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendRequest {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub content: String,
    #[serde(default = "default_delivery")]
    pub delivery: DeliveryMode,
    /// 用户粘贴的段（每段一个块，投给模型时照原文）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pastes: Vec<String>,
    /// 用户拖进来的文件（已经存进附件仓库）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SendAttachment>,
    /// 画布按钮放进输入框的那一句带着画布身份（id + 标题）；用户自己写的不带。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canvas: Option<SendCanvas>,
}

/// 发送消息时引用的一个附件（服务端解析成只读副本路径）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendAttachment {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

/// 画布放进输入框的那一句带来的画布（页面从日志的 meta.canvas 拿 id 和标题）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SendCanvas {
    pub id: String,
    pub title: String,
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

/// 取消当前轮（不丢 pending inbox）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CancelRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CancelResponse {
    pub cancelled: bool,
}

/// 一条答案（{id, selected[], custom?}）。
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

/// 关掉/取消待答问题（「关掉不答」）。一次撤掉当前全部待答问题：
/// 等待中的批以 DSH ASK_ABORTED 工具错误收尾、本轮继续；重启恢复的批先补写已收答案，
/// 再写 question/cancelled 并移除。DSH 没有对应的 Remote 方法（关闭面板不发请求，靠
/// timed wait 超时；见 user-questions README 第 35、40 行），
/// 这里是按撤销语义做的适配，不假装与 DSH 相同。
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
    /// 会话统计（轮/步/token/缓存/上下文/花费）；裸宿主为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<stats::SessionStats>,
    /// USD→CNY 汇率与拉到时间；裸宿主为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<fx::FxInfo>,
    /// 币种（settings.json：usd/cny）；裸宿主为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// 会话的工作目录（工具的路径相对它解析）；页面用它把工作过程里的路径显示成相对路径。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
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
    /// 根不可用时没有 ready 帧，页面靠这里的 host.home 缩写说明页里的路径。
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

/// 失败原因分类：页面失败块按这个分类写文案，不按 code 猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureCause {
    /// 凭证问题（401 / 没配 key）；Keyring 问题另算。
    Credential,
    /// 模型侧失败（重试用完/超时/服务错误等）。
    Model,
    /// Keyring 读不出/无法签发；页面失败块说 Keyring 原因、指去设置。
    Keyring,
    /// 模型在设置里被屏蔽：失败块给「换模型」和「去设置」，不重试。
    Masked,
}

/// 服务事件：严格来自会话日志事实（seq 连续、type 是日志事件类型、data 是事件字段）。
/// cause 是只增的派生字段：只有 turn/end 的 error 事件带，取值来自日志里的 code，
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

/// 设置页读模型连接（空请求体）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelSettingsRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelSettingsResponse {
    /// Keyring 视图：旧字段原样保留，页面照旧读。
    pub settings: credentials::KeyringView,
    /// 屏蔽规则的整表（保存时的顺序）。
    #[serde(rename = "disabledModels")]
    pub disabled_models: Vec<String>,
    /// settings.json 读不出来、而且这个进程从没读到过：不知道哪些被屏蔽（所有模型都拒）。
    /// 坏文件但上次读到过为 false：沿用上次读到的表。
    #[serde(rename = "disabledUnknown")]
    pub disabled_unknown: bool,
    /// 与 disabledModels 等长同序：每条规则现在命中的「连接/模型」；一条都不命中的给空表。
    #[serde(rename = "disabledHits")]
    pub disabled_hits: Vec<Vec<String>>,
    /// 每个连接现在存下的厂商列表，键是连接 id（没存过的连接不出现）。
    pub listed: std::collections::BTreeMap<String, ListedModels>,
}

/// 一个连接存下的厂商模型列表（获取时按连接存；页面小标签用它）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ListedModels {
    /// 存下这份列表的时间（Unix 秒）。
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
    pub models: Vec<model::catalog::ModelInfo>,
}

/// 整表替换屏蔽规则（空表 = 都不屏蔽）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisabledModelsSetRequest {
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisabledModelsSetResponse {
    /// 规范化后的整表：去首尾空白、丢空规则、不分大小写去重，保持原来的顺序。
    pub models: Vec<String>,
}

/// 查一条规则现在匹配到哪些模型、谁在用。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DisabledModelsCheckRequest {
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisabledModelsCheckResponse {
    /// 现在匹配到的「连接/模型」。
    pub hits: Vec<String>,
    pub users: Vec<DisabledModelUser>,
}

/// 谁在用（只列当前 Kind 对话与新会话；任务、角色、日历以后加）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DisabledModelUser {
    /// 当前 Kind 对话；`running` = 有一轮正在跑。
    KindChat { running: bool },
    /// 新会话会用上一次的选择。
    NewSessions,
}

/// 存/换连接；apiKey 只在请求体里，任何响应不带。
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
    pub auth_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionSaveResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 这一次 `available-models` 是页面「看一眼」，还是用户按了「获取可用模型」。
///
/// 看一眼只补目录字段（价格、窗口、档）：不写盘，也不挪动「新」的基准；获取成功才把这次
/// 的厂商列表存下来。两者混在一起的话，打开选择器或设置页就会把列表存掉，用户看不到「新」。
/// 缺这个字段的旧页面按「获取」处理：它们只有「获取」这一个入口，行为不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum AvailableModelsMode {
    /// 用户按了「获取可用模型」：厂商返回成功就存下这次的列表，下一次拿它当基准。
    #[default]
    Fetch,
    /// 页面只是要目录字段（价格/窗口/档）：只读，不落盘。
    Query,
}

/// 获取某连接的可用模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct AvailableModelsRequest {
    pub id: String,
    /// 看一眼还是获取；见 [`AvailableModelsMode`]。
    #[serde(default)]
    pub mode: AvailableModelsMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableModelsResponse {
    pub models: Vec<AvailableModelRow>,
    /// 厂商 /v1/models 是否成功；失败时只给目录/用户模型。
    pub live: bool,
}

/// 可用模型的一行：目录字段（camelCase）不变，另加这次的「新」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AvailableModelRow {
    #[serde(flatten)]
    pub model: model::catalog::ModelInfo,
    /// 厂商这次给的模型里，上一次存下的列表没见过的那个（从没存过时：内置提供方
    /// 和目录比，自定义地址一条都不标）。
    pub fresh: bool,
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

/// 验证一个连接（空 = 用注册器里的 endpoint/key 发最小请求）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionVerifyRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionVerifyResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 查询连接余额/套餐余量（查不到不报错，字段为空）。
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

/// 写非密钥设置（settings.json）；只碰非密钥设置，不碰 Keyring/连接。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct SettingsUpdateRequest {
    /// 币种（usd/cny）；None = 不改。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SettingsUpdateResponse {
    pub currency: String,
}

/// 页面异常 / 事件流重连上报：只记 web 的 ops，不进会话日志；正文限长由载体截。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorReportRequest {
    /// 上报种类；未知值按 page-error 记。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 上报结果（页面不读内容；ok = 已记下）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MonitorReportResponse {
    pub ok: bool,
}

/// 选模型（连接、模型、想要的档）；只影响下一轮（D26）。
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
    /// 这一轮实际会发的档（没有想要的档时是最近档；页面据此显示并告诉用户）。
    #[serde(rename = "effortActual")]
    pub effort_actual: String,
}

/// 存一份文字附件（data 是 base64 的原始字节）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentPutRequest {
    pub name: String,
    /// base64 编码的内容；不规范直接拒收（request/invalid）。
    pub data: String,
}

/// 存好后的附件 id（sha256:<64hex>）与字节数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentPutResponse {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

/// 按 id 读回附件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentGetRequest {
    pub id: String,
}

/// 读回的附件（文字文件附全文；二进制不进仓库，text 为 None 兼容旧数据）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AttachmentGetResponse {
    pub name: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// @ 文件候选（query 是 @ 后面那段；空就是列前面的）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FileReferencesListRequest {
    #[serde(default)]
    pub query: String,
}

/// 一个候选（相对工作目录的路径 + 内容字节数）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileReferenceEntry {
    pub path: String,
    pub bytes: u64,
}

/// @ 文件候选列表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct FileReferencesListResponse {
    pub references: Vec<FileReferenceEntry>,
}

/// 编辑连接，只改 endpoint/model（不换 key）。
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
    pub auth_header: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionUpdateResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 撤销 key（连接保留，之后签发失败）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionRevokeRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectionRevokeResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 服务协议版本：只增不改的对外面每次扩展都记在这里，Web ready 帧里带上。
///
/// - 4：第一版；
/// - 5：settings-update（写非密钥设置 currency）。
/// - 6：attachment-put / attachment-get / file-references-list 三个 op，
///   以及 send 的 pastes / attachments 两个字段（后两项随后实现，形状已定）。
/// - 7：tool-call-delta + send.canvas（send.canvas 先加字段）。
/// - 8：屏蔽的模型——`disabled-models-set`、`disabled-models-check`；
///   `model-settings` 的 disabledModels / disabledUnknown / disabledHits / listed；
///   `available-models` 每行的 fresh；`FailureCause` 的 masked；
///   新错误码 settings/unreadable（码表 16）。全是只增字段/值，旧页忽略。
///
/// 连接上的旧价格乘数后来整条删掉，不升版本：两个请求里的这个字段本来就可省略，旧页再发
/// 由 serde 当未知字段忽略；响应里少了它，旧页读到 undefined 也不会拿去定价。
pub const PROTOCOL_VERSION: u64 = 8;

/// 根会话不可用的种类：服务照常起，ready 报出来，方法一律拒绝。
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

/// 根会话状态：ready 里原样给页面，用于五种说明页。
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

/// 上下文占用：服务从日志里的 usage + 模型配置的窗口算，页面不推算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsage {
    pub used: u64,
    pub window: u64,
}

/// 宿主机的位置（ready 帧的 host）：页面只用 home 把路径缩写成 ~/…。
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
    /// ready 帧加 host: {home}；不加 clientId。
    pub host: HostLocation,
}
