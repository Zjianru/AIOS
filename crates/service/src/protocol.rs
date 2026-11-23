//! 线上协议形状：调用面 Call/Response、事件面 ServiceEvent 及其请求/响应类型。
//! 只放数据形状和序列化，不放行为。

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{fx, quota, stats};

/// 服务对象：固定 Kind 根会话（16b 换成根会话实体）；第一版用会话 id 表达。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct Object {
    #[serde(rename = "sessionId")]
    pub session_id: String,
}

/// 投递方式（服务协议里用稳定的 kebab-case 名）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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
    /// 只读打开一个任务当前 Attempt 的会话（头部 + 一页事件）。
    TaskRead(TaskReadRequest),
    /// 读一份交付记录（不挂 TaskRead；它被轮询，改动清单要读会话日志）。
    TaskDelivery(TaskDeliveryRequest),
    /// 停一个任务（和 Kind 的 task_stop 走同一个服务方法）。
    TaskStop(TaskStopRequest),
    /// 停下任务当前这一轮：不结束 Attempt，下一句话仍在这个 Attempt 里接着做。
    TaskInterrupt(TaskInterruptRequest),
    /// 在任务对话里说一句：记输入、排 delivery 和 notice。
    TaskSay(TaskSayRequest),
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
    /// 获取可用模型（目录 + 厂商列表）。
    AvailableModels(AvailableModelsRequest),
    KeyringRelocate(KeyringRelocateRequest),
    KeyringReopen(KeyringReopenRequest),
    KeyringRestart(KeyringRestartRequest),
    Issues(IssuesRequest),
    FirstUse(FirstUseRequest),
    /// 选模型（下一轮生效，当前轮不换）。
    SessionModelSelect(SessionModelSelectRequest),
    /// 确认卡上的按钮：确认某一项，或者暂不。
    CardAct(CardActRequest),
    /// T077.5：任务页菜单直接走登记动作：先读事实，再执行。
    ActionDescribe(crate::protocol::ActionDescribeRequest),
    ActionAct(crate::protocol::ActionActRequest),
    /// 查询连接余额/套餐余量。
    ConnectionQuota(ConnectionQuotaRequest),
    /// 设置页「联网搜索」：读 Exa 凭证（不含 key 正文）。
    ExaKey(ExaKeyRequest),
    /// 存/换 Exa 的 key。
    ExaKeySave(ExaKeySaveRequest),
    /// 用已存的 key 向 Exa 发一次最小搜索，结果写回 Keyring。
    ExaKeyCheck(ExaKeyRequest),
    /// 删 Exa 的 key。
    ExaKeyDelete(ExaKeyRequest),
    /// 写非密钥设置（currency；只写 settings.json）。
    SettingsUpdate(SettingsUpdateRequest),
    /// 存一份文字附件（只读副本），返回 id。
    AttachmentPut(AttachmentPutRequest),
    /// 按 id 读回附件（卡片「点开看全文」）。
    AttachmentGet(AttachmentGetRequest),
    /// @ 文件候选（按会话工作目录）。
    FileReferencesList(FileReferencesListRequest),
    /// T053：用户在门结果卡上判定（内部操作；Kind 没有判定工具）。
    GateJudge(GateJudgeRequest),
    /// T053：Kind 让任务在原要求里接着做（需修正/上游变了/报告卡住）。
    TaskContinue(TaskContinueRequest),
    /// T055：工作页/「现在」共用的工作快照（全部任务事实 + 五种「等你」项）。
    Work(WorkRequest),
    /// 项目列表/创建/更新，任务归属。
    #[serde(rename = "project_list")]
    ProjectList(ProjectListRequest),
    #[serde(rename = "project_create")]
    ProjectCreate(ProjectCreateRequest),
    #[serde(rename = "project_update")]
    ProjectUpdate(ProjectUpdateRequest),
    #[serde(rename = "task_set_project")]
    TaskSetProject(TaskSetProjectRequest),
    /// 监控消耗：按用户时区口径现算全部任务/模型/工具/命令合计。
    MonitorSnapshot(MonitorSnapshotRequest),
    /// 监控诊断：一个任务/Attempt 的链路事实；Kind 不诊断。
    MonitorTask(MonitorTaskRequest),
    /// T055.2：勾选后的组接受（每项带勾选那一刻看到的报告与版本）。
    TaskAcceptMany(TaskAcceptManyRequest),
    /// 记忆页读取（M02/M05/M11）：关于你/条目/日记/隔离区/做梦/搜索/小卡状态。
    MemoryRead(MemoryReadRequest),
    /// 记忆变更（M01/M03/M04/M10/M12/M13/M14；一次只改一个对象/一处动作）。
    MemoryAction(MemoryActionRequest),
    /// 日历读取。
    CalendarRead(CalendarReadRequest),
    /// 日历动作。
    CalendarAction(CalendarActionRequest),
}

/// 调用的结果（与 Call 一一对应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "method", rename_all = "kebab-case")]
#[allow(clippy::large_enum_variant)]
pub enum Response {
    Send(SendResponse),
    Cancel(CancelResponse),
    Answer(AnswerResponse),
    DismissQuestions(DismissQuestionsResponse),
    Compact(CompactResponse),
    Status(StatusResponse),
    Follow(FollowResponse),
    Events(EventsResponse),
    /// 任务对话的头部与一页事件。
    TaskRead(TaskReadResponse),
    /// 交付记录：任务页和 Kind 读同一份派生。
    TaskDelivery(TaskDeliveryResponse),
    /// 停任务的结果：是否真的停下、停下后的阶段、给页面的固定说明。
    TaskStop(TaskStopResponse),
    /// 停下这一轮的结果：是否真的中断了，以及给页面的固定说明。
    TaskInterrupt(TaskInterruptResponse),
    /// 说一句的结果：记下的输入 id，和没在跑时新开的 Attempt 编号。
    TaskSay(TaskSayResponse),
    MonitorReport(MonitorReportResponse),
    Recheck(RecheckResponse),
    ModelSettings(ModelSettingsResponse),
    DisabledModelsSet(DisabledModelsSetResponse),
    DisabledModelsCheck(DisabledModelsCheckResponse),
    ConnectionSave(ConnectionSaveResponse),
    ConnectionDelete(ConnectionDeleteResponse),
    ConnectionUpdate(ConnectionUpdateResponse),
    ConnectionRevoke(ConnectionRevokeResponse),
    /// 连接余额/套餐余量。
    ConnectionQuota(ConnectionQuotaResponse),
    /// Exa 凭证的视图（四个操作同一形状）。
    ExaKey(ExaKeyResponse),
    ExaKeySave(ExaKeyResponse),
    ExaKeyCheck(ExaKeyResponse),
    ExaKeyDelete(ExaKeyResponse),
    /// 写非密钥设置。
    SettingsUpdate(SettingsUpdateResponse),
    AvailableModels(AvailableModelsResponse),
    KeyringRelocate(KeyringRelocateResponse),
    KeyringReopen(KeyringReopenResponse),
    KeyringRestart(KeyringRestartResponse),
    Issues(IssuesResponse),
    FirstUse(FirstUseResponse),
    SessionModelSelect(SessionModelSelectResponse),
    /// 确认卡动作的受理结果（已经有决定时原样返回那条决定）。
    CardAct(CardActResponse),
    ActionDescribe(crate::protocol::ActionDescribeResponse),
    ActionAct(crate::protocol::ActionActResponse),
    /// 附件仓库。
    AttachmentPut(AttachmentPutResponse),
    AttachmentGet(AttachmentGetResponse),
    /// @ 文件候选。
    FileReferencesList(FileReferencesListResponse),
    /// 门判定的受理结果：记下的判定，或「已经变了」。
    GateJudge(GateJudgeResponse),
    /// Kind 让任务接着做的受理结果：新 Attempt 编号。
    TaskContinue(TaskContinueResponse),
    /// T055：工作页/「现在」共用的工作快照。
    Work(WorkResponse),
    #[serde(rename = "project_list")]
    ProjectList(ProjectListResponse),
    #[serde(rename = "project_create")]
    ProjectCreate(ProjectCreateResponse),
    #[serde(rename = "project_update")]
    ProjectUpdate(ProjectUpdateResponse),
    #[serde(rename = "task_set_project")]
    TaskSetProject(TaskSetProjectResponse),
    MonitorSnapshot(MonitorSnapshotResponse),
    MonitorTask(MonitorTaskResponse),
    /// T055.2：组接受的结果（通过的一起记，没通过的逐项回原因）。
    TaskAcceptMany(TaskAcceptManyResponse),
    /// 记忆页读取的完整视图。
    MemoryRead(MemoryReadResponse),
    /// 记忆变更的受理结果（新版本号 + 给页面的一句话）。
    MemoryAction(MemoryActionResponse),
    /// 日历读取：约定 + 运行记录 + 某周计划点。
    CalendarRead(CalendarReadResponse),
    /// 日历动作：暂停/恢复/立即跑一次/取消。
    CalendarAction(CalendarActionResponse),
}

/// 发消息（DSH session-controller 的 prompt）。requestId 是幂等键：同一 id 只进一次。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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
    /// T054.2：结果卡「要求修改」带进输入框的那张卡（页面从日志块画成卡，模型看一行文字）。
    #[serde(
        default,
        rename = "resultCard",
        skip_serializing_if = "Option::is_none"
    )]
    pub result_card: Option<SendResultCard>,
}

/// T054.2：随消息带进 Kind 输入框的一张任务结果卡。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct SendResultCard {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub title: String,
    /// 这张卡是哪一版要求的结果（1 起）。
    pub version: u32,
}

/// 发送消息时引用的一个附件（服务端解析成只读副本路径）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SendAttachment {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

/// 画布放进输入框的那一句带来的画布（页面从日志的 meta.canvas 拿 id 和标题）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SendCanvas {
    pub id: String,
    pub title: String,
}

fn default_delivery() -> DeliveryMode {
    DeliveryMode::FollowUp
}

/// 发消息的受理回执（重复 requestId 返回原始受理，duplicate=true）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SendResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub accepted: bool,
    pub duplicate: bool,
}

/// 取消当前轮（不丢 pending inbox）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct CancelRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CancelResponse {
    pub cancelled: bool,
}

/// 一条答案（{id, selected[], custom?}）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ServiceAnswer {
    #[serde(rename = "questionId")]
    pub question_id: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// 回答整批（一次调用覆盖一批问题；逐题走 Agent::answer，全部被接受才算成功）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AnswerRequest {
    pub answers: Vec<ServiceAnswer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AnswerResponse {
    pub applied: usize,
}

/// 关掉/取消待答问题（「关掉不答」）。一次撤掉当前全部待答问题：
/// 等待中的批以 DSH ASK_ABORTED 工具错误收尾、本轮继续；重启恢复的批先补写已收答案，
/// 再写 question/cancelled 并移除。DSH 没有对应的 Remote 方法（关闭面板不发请求，靠
/// timed wait 超时；见 user-questions README 第 35、40 行），
/// 这里是按撤销语义做的适配，不假装与 DSH 相同。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct DismissQuestionsRequest {}

/// 关掉不答的回执：是否确实有待答问题被撤掉（没有待答问题时 false）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct DismissQuestionsResponse {
    pub dismissed: bool,
}

/// 手动压缩（DSH command-compact）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct CompactRequest {
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "commandId")]
    pub command_id: Option<String>,
}

/// 压缩计数（有可压缩历史才 Some）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CompactCounts {
    #[serde(rename = "shadowedItems")]
    pub shadowed_items: usize,
    #[serde(rename = "shadowedTokens")]
    pub shadowed_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CompactResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted: Option<CompactCounts>,
}

/// 被动状态（DSH state 的第一版子集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct StatusRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct FollowRequest {
    /// 调用方已持有的最后序号（0 = 从头补齐）。
    #[serde(default, rename = "lastSeq")]
    pub last_seq: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct FollowResponse {
    /// seq > last_seq 的事件（按日志顺序、连续、不重复）。
    pub events: Vec<ServiceEvent>,
    /// 补齐后的最后序号（events 为空时等于请求的 last_seq）。
    #[serde(rename = "lastSeq")]
    pub last_seq: u64,
}

/// events（DSH :540）：往回翻一页日志，按轮对齐、有预算（默认 50/2/500）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct RecheckRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct RecheckResponse {
    pub root: RootState,
    /// 根不可用时没有 ready 帧，页面靠这里的 host.home 缩写说明页里的路径。
    #[serde(default)]
    pub host: HostLocation,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct EventsResponse {
    /// 这一页事件（按日志顺序；页首通常是 turn/start，除非预算或日志耗尽先到）。
    pub events: Vec<ServiceEvent>,
    /// 还有更早的事件（下一轮用 beforeSeq = 页首 seq 往回翻，不含本条）。
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

/// task-read：任务对话的头部（任务事实；从 tasks 库读，不从会话日志推）。
///
/// 这是页面头部的全部事实：标题、要求、DoD、cwd、冻结的模型、工具名单、Attempt 数。
/// T051：当前 Attempt 最近一份报告的状态（页头状态词用；词本身由页面 taskStatusOf 一处算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum TaskReadReportState {
    Ready,
    Blocked,
    Failed,
    Waiting,
}

/// T051：报告是谁写的（页头把 package 的 blocked 与 agent 的 blocked 分开展示）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum TaskReadReportOrigin {
    Package,
    Agent,
    Executor,
    /// T057：程序门执行者写的报告。
    Program,
}

/// T051：当前 Attempt 最近一份报告的缩略事实；没报告时字段为 None。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskReadReport {
    pub state: TaskReadReportState,
    pub origin: TaskReadReportOrigin,
    /// T056.2：最近报告的摘要/问题（账本 UI 的进展和待答从这里读）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub summary: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub question: Option<String>,
    /// T056.2 补审：这份报告属于第几次 Attempt（账本进展显示用）。
    #[serde(default)]
    pub attempt: u64,
    /// T057：程序门检查的逐条结果；普通报告为空。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<TaskReadGateCheck>,
}

/// T057：程序门任务页的一条命令。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGateCommand {
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
}

/// T057：程序门任务页的一条检查结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGateCheck {
    pub index: usize,
    pub command: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub outcome: String,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spill: Option<String>,
}

/// T059 F4：里程碑门的一次用户判定。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGateVerdict {
    /// 候选报告排第几次；找不到那份报告就不写（版本号不是次数）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub candidate_ordinal: Option<u64>,
    pub conclusion: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    pub at: i64,
    pub judged_by: String,
}

/// T059 F4：门任务的一次核查 Attempt（摘要 + 时间）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGateAttempt {
    pub attempt: u64,
    pub at: i64,
    pub summary: String,
}

/// T057：程序门任务页头部的事实。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGate {
    pub commands: Vec<TaskReadGateCommand>,
    pub commands_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged_by: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    /// T057：历次程序检查（最新在前）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runs: Vec<TaskReadGateRun>,
    /// T059 F4：用户判门历次判定（最新在前）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub verdicts: Vec<TaskReadGateVerdict>,
    /// T059 F4：这次/历次核查 Attempt（最新在前）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attempts: Vec<TaskReadGateAttempt>,
}

/// T057：任务页一次程序检查（报告 + 判定）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGateRun {
    pub attempt: u64,
    pub commands_version: u32,
    pub conclusion: String,
    pub at: i64,
    pub checks: Vec<TaskReadGateCheck>,
}

/// T058：被把关任务页看到的守卫门事实（判定/命令版本变时让交付记录重读）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadGuardingGate {
    pub id: String,
    pub title: String,
    pub program: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub commands_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<i64>,
}

/// T056.2：任务对话页账本块的只读投影；进展/待答不在里面（从 report 读）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadLedger {
    pub constraints: Vec<TaskReadLedgerConstraint>,
    pub notes: Vec<TaskReadLedgerNote>,
    pub history: Vec<TaskReadLedgerHistory>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadLedgerConstraint {
    pub text: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadLedgerNote {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub attempt: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskReadLedgerHistory {
    pub id: String,
    pub kind: String,
    pub text: String,
    pub attempt: u64,
    #[serde(rename = "closedAt")]
    pub closed_at: i64,
    #[serde(rename = "closedWhy")]
    pub closed_why: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskReadHeader {
    pub id: String,
    pub title: String,
    pub requirement: String,
    pub dod: String,
    pub cwd: String,
    /// 当前要求版本的写入范围，相对任务 cwd 显示；None = 只读调查任务。
    #[serde(
        default,
        rename = "writeScope",
        skip_serializing_if = "Option::is_none"
    )]
    pub write_scope: Option<String>,
    /// 创建任务时冻结的 bash 开关；旧任务行按 false 读。
    #[serde(default)]
    pub bash: bool,
    /// 任务取消时间；None = 没取消。
    #[serde(
        default,
        rename = "cancelledAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub cancelled_at: Option<i64>,
    /// 所属项目；None = 个人。
    #[serde(default, rename = "projectId", skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// 目录来源：user / project / conversation；旧行 None。
    #[serde(default, rename = "cwdSource", skip_serializing_if = "Option::is_none")]
    pub cwd_source: Option<String>,
    /// 这是一张任务图的总任务；改归属时整张图一起改。
    #[serde(default, rename = "graphRoot")]
    pub graph_root: bool,
    /// 这张任务属于一张任务图（总任务或子任务）；改归属时整张图一起改。
    #[serde(default, rename = "graphMember")]
    pub graph_member: bool,
    /// 创建任务时冻结的模型（只给显示名，切换模型不影响它）。
    pub model: String,
    pub tools: Vec<String>,
    pub attempts: u64,
    /// 这个 Attempt 的会话现在真的在跑一轮（不是只开着等报告）；任务菜单只在为 true 时可点。
    #[serde(default)]
    pub running: bool,
    /// 报告后保留的宿主还有后台命令在跑（只停得掉、不再叫醒模型）。
    #[serde(default, rename = "backgroundRunning")]
    pub background_running: bool,
    /// 最近的 Attempt 以「停止」结束、之后没有新的 Attempt：页头固定显示「已停止」。
    #[serde(default)]
    pub stopped: bool,
    /// 最近的 Attempt 以「打断」结束、之后没有新的 Attempt：页头固定显示「被打断了」（D84）。
    #[serde(default)]
    pub interrupted: bool,
    /// 这次任务被启动扫描打断、给 Kind 的 restart 汇报还没送出去：页头显示
    /// 「待 Kind 核实，尚未接续」；送到之后横幅消失，任务仍是「被打断了」。
    #[serde(default, rename = "restartPending")]
    pub restart_pending: bool,
    /// 当前 Attempt 最近一份报告的写入时间（unix 毫秒）；None = 这次 Attempt 还没交报告。
    /// 终态报告后执行者会立刻关掉会话，日志可能没有 turn/end：页面据此判断最后一轮不再在跑。
    #[serde(
        default,
        rename = "reportedAt",
        skip_serializing_if = "Option::is_none"
    )]
    pub reported_at: Option<i64>,
    /// T051：当前 Attempt 最近一份报告的 { state, origin }；None = 还没交报告。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report: Option<TaskReadReport>,
    /// T051：写任务、报告 ready、报告的要求版本是当前版本（和 relay 加 card_state:review 同一判断）。
    #[serde(default)]
    pub review: bool,
    /// T051：用户接受记录绑的就是当前版本和这份报告。
    #[serde(default)]
    pub accepted: bool,
    /// T057：程序门事实（命令/版本/最近判定）；用户判门或普通任务为空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<TaskReadGate>,
    /// T057：这一轮程序检查在跑（任务页显示「程序在跑」并在跑哪条）。
    #[serde(default)]
    pub program_running: bool,
    /// T057：正在跑第几条（0 起）；没在跑为 None。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program_index: Option<usize>,
    /// T058：被这个任务的门（被把关任务才有）；用于页头/交付记录在门判定变化时重读。
    #[serde(
        default,
        rename = "guardingGate",
        skip_serializing_if = "Option::is_none"
    )]
    pub guarding_gate: Option<TaskReadGuardingGate>,
}

/// task-read：打开任务当前 Attempt 的会话，拿头部和一页事件。
///
/// `beforeSeq` 的语义和 events 一样：页的末端（不含本条）；省略 = 日志末端之后。
/// `lastSeq` 始终是这条会话日志的末端序号（=0 表示日志为空），往上翻页不改它。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskReadRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "beforeSeq")]
    pub before_seq: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct TaskReadResponse {
    pub task: TaskReadHeader,
    pub events: Vec<ServiceEvent>,
    #[serde(rename = "lastSeq")]
    pub last_seq: u64,
    /// T051：任务当前 Attempt 的会话统计（和 status 的 stats 同一形状、同一函数算；页面只显示）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stats: Option<stats::SessionStats>,
    /// USD→CNY 汇率与拉到时间（同 status；页面按服务端给的换算显示花费）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fx: Option<fx::FxInfo>,
    /// 币种（settings.json：usd/cny）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// T056.2：账本（约束、在用叙述条、历史）；旧任务为空数组，页面不显示。
    #[serde(default)]
    pub ledger: TaskReadLedger,
}

/// T058：读一份交付记录；不挂 TaskRead，因为它只在需要时读一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskDeliveryRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// T058：交付记录；门任务、总任务、还没交过 Ready 的任务返回 None。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskDeliveryResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<crate::delivery::DeliveryRecord>,
}

/// T055：读一份工作快照；没有参数（窗口用浏览器本地时间算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize, TS)]
pub struct WorkRequest {}

/// T055：工作页 /「现在」共用的一份工作快照。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkResponse {
    /// 服务算这份快照的时刻（unix 毫秒）。
    pub generated_at: i64,
    /// 上板的任务事实（按 created_at 升序）：已确认图的子任务 + 单派任务；
    /// 总任务、未确认图和取消图的子任务都不上。
    pub tasks: Vec<crate::work::TaskFact>,
    /// 五种「等你」项，按固定顺序、同种新的在上。
    pub wait: Vec<crate::work::WaitItem>,
    /// 已确认、没取消的任务图；服务算好每条线的 kind/state/program，页面只排和画。
    pub graphs: Vec<crate::work::WorkGraphFact>,
}

/// 项目视图。code（首字）和颜色由页面按列表顺序算，不进协议/库。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ProjectListRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectListResponse {
    pub projects: Vec<ProjectView>,
}

/// 创建项目和更新项目共用的输入形状；更新多一个 id。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateRequest {
    pub name: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCreateResponse {
    pub project: ProjectView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdateRequest {
    pub id: String,
    pub name: String,
    pub goal: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dir: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ProjectUpdateResponse {
    pub project: ProjectView,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskSetProjectRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// None = 归个人；任务图里改任一任务整张图一起改。
    #[serde(rename = "projectId", default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskSetProjectResponse {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(rename = "projectId", default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// 实际改到的任务 id（任务图是整张图）。
    pub changed: Vec<String>,
}

/// T055.2：组接受的一项（页面勾选时存下的报告与版本）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskAcceptManyRequest {
    pub picks: Vec<crate::work::WorkPick>,
}

/// T055.2：组接受里一项的结果；refused 是给页面直接显示的原因原话。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskAcceptManyOutcome {
    pub task_id: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
}

/// T055.2：一次组接受的结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct TaskAcceptManyResponse {
    /// 这一批接受的时刻（unix 毫秒）。
    pub accepted_at: i64,
    /// 按勾选顺序，一项一条。
    pub outcomes: Vec<TaskAcceptManyOutcome>,
}

/// 停法：立即停，或做完手上这一步再停（D81）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum StopMode {
    Now,
    AfterReport,
}

/// 停一个任务；和 Kind 的 task_stop 走同一个服务方法。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskStopRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub mode: StopMode,
}

/// 停下任务当前这一轮；任务和 Attempt 都留着。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskInterruptRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
}

/// 停下这一轮的结果。没在跑时 interrupted 为 false，message 是固定原因。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskInterruptResponse {
    pub interrupted: bool,
    /// 给页面的固定说明。
    pub message: String,
}

/// 任务对话里说一句。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct TaskSayRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub text: String,
    /// 用户粘贴的段；和 Kind 对话的 SendRequest 同名同形。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub pastes: Vec<String>,
    /// 用户拖进来的文件（已经存进附件仓库）；规则和 Kind 对话一样。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<SendAttachment>,
}

/// 说一句的结果：输入 id 和新 Attempt 编号（在跑时没有新 Attempt）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskSayResponse {
    #[serde(rename = "inputId")]
    pub input_id: String,
    #[serde(
        default,
        rename = "newAttempt",
        skip_serializing_if = "Option::is_none"
    )]
    pub new_attempt: Option<u64>,
}

/// 停任务的结果。没在跑时 stopped 为 false，message 是固定原因；
/// 收尾停插进去之后 stopped 为 true，message 是停法和当前阶段。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct TaskStopResponse {
    pub stopped: bool,
    /// 停下之后的阶段（needs_conditions 等），没停下时是当前阶段。
    pub stage: String,
    /// 给页面和 Kind 的完整说明；原因句是固定的。
    pub message: String,
}

/// 失败原因分类：页面失败块按这个分类写文案，不按 code 猜。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ServiceEvent {
    pub seq: u64,
    /// 日志事件发生时间（unix 毫秒；页面工作过程开始时间从这里取）。
    pub time: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    #[ts(type = "Record<string, unknown>")]
    pub data: serde_json::Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cause: Option<FailureCause>,
}

/// 设置页读模型连接（空请求体）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ModelSettingsRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ListedModels {
    /// 存下这份列表的时间（Unix 秒）。
    #[serde(rename = "fetchedAt")]
    pub fetched_at: u64,
    pub models: Vec<model::catalog::ModelInfo>,
}

/// 整表替换屏蔽规则（空表 = 都不屏蔽）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DisabledModelsSetRequest {
    #[serde(default)]
    pub models: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct DisabledModelsSetResponse {
    /// 规范化后的整表：去首尾空白、丢空规则、不分大小写去重，保持原来的顺序。
    pub models: Vec<String>,
}

/// 查一条规则现在匹配到哪些模型、谁在用。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct DisabledModelsCheckRequest {
    #[serde(default)]
    pub pattern: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct DisabledModelsCheckResponse {
    /// 现在匹配到的「连接/模型」。
    pub hits: Vec<String>,
    pub users: Vec<DisabledModelUser>,
}

/// 谁在用（只列当前 Kind 对话与新会话；任务、角色、日历以后加）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DisabledModelUser {
    /// 当前 Kind 对话；`running` = 有一轮正在跑。
    KindChat { running: bool },
    /// 新会话会用上一次的选择。
    NewSessions,
}

/// 存/换连接；apiKey 只在请求体里，任何响应不带。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionSaveResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 这一次 `available-models` 是页面「看一眼」，还是用户按了「获取可用模型」。
///
/// 看一眼只补目录字段（价格、窗口、档）：不写盘，也不挪动「新」的基准；获取成功才把这次
/// 的厂商列表存下来。两者混在一起的话，打开选择器或设置页就会把列表存掉，用户看不到「新」。
/// 缺这个字段的旧页面按「获取」处理：它们只有「获取」这一个入口，行为不变。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
#[serde(rename_all = "kebab-case")]
pub enum AvailableModelsMode {
    /// 用户按了「获取可用模型」：厂商返回成功就存下这次的列表，下一次拿它当基准。
    #[default]
    Fetch,
    /// 页面只是要目录字段（价格/窗口/档）：只读，不落盘。
    Query,
}

/// 获取某连接的可用模型。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default, TS)]
pub struct AvailableModelsRequest {
    pub id: String,
    /// 看一眼还是获取；见 [`AvailableModelsMode`]。
    #[serde(default)]
    pub mode: AvailableModelsMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AvailableModelsResponse {
    pub models: Vec<AvailableModelRow>,
    /// 厂商 /v1/models 是否成功；失败时只给目录/用户模型。
    pub live: bool,
}

/// 可用模型的一行：目录字段（camelCase）不变，另加这次的「新」。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AvailableModelRow {
    #[serde(flatten)]
    pub model: model::catalog::ModelInfo,
    /// 厂商这次给的模型里，上一次存下的列表没见过的那个（从没存过时：内置提供方
    /// 和目录比，自定义地址一条都不标）。
    pub fresh: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionDeleteRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionDeleteResponse {
    pub deleted: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct KeyringRelocateRequest {
    pub to: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct KeyringRelocateResponse {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct KeyringReopenRequest {
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct KeyringReopenResponse {}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct KeyringRestartRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct KeyringRestartResponse {
    pub settings: credentials::KeyringView,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct IssuesRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct IssuesResponse {
    #[serde(rename = "issuedToday")]
    pub issued_today: u64,
    pub issues: Vec<credentials::IssueRecord>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct FirstUseRequest {}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct FirstUseResponse {
    #[serde(rename = "firstUse")]
    pub first_use: bool,
    #[serde(rename = "keyFile")]
    pub key_file: credentials::KeyFileInfo,
}

/// 查询连接余额/套餐余量（查不到不报错，字段为空）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ConnectionQuotaRequest {
    pub id: String,
}

/// 余额（厂商币种）与套餐余量（MiniMax：5 小时窗口/每周剩余百分比）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionQuotaResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balance: Option<quota::BalanceInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<quota::PlanQuota>,
    /// 查询时间（Unix 秒）。
    #[serde(rename = "checkedAt")]
    pub checked_at: u64,
}

/// 设置页「联网搜索」：读/验证/删 Exa 凭证（空请求体）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExaKeyRequest {}

/// 存/换 Exa 的 key；只在这一次请求体里，任何响应不带正文。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExaKeySaveRequest {
    pub key: String,
}

/// Exa 凭证的视图：存没存、保存时间、上次验证结果。没有 key 正文。
pub type ExaKeyResponse = credentials::ToolKeyView;

/// 写非密钥设置（settings.json）；只碰非密钥设置，不碰 Keyring/连接。
///
/// 全部字段为 None 时只读当前值（设置页打开时用），不写文件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS)]
pub struct SettingsUpdateRequest {
    /// 币种（usd/cny）；None = 不改。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    /// 全局暂停；None = 不改。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    /// 席位（1–16）；None = 不改。越界由服务拒绝，不写文件。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seats: Option<u8>,
    /// 默认角色的 bash 开关（D85）；None = 不改。只影响之后派的任务。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bash: Option<bool>,
}

/// 设置 › 执行 的当前值（页面读回用；席位已夹在 1–16）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ExecSettingsView {
    pub paused: bool,
    pub seats: u8,
    /// 默认角色的 bash 开关（D85）；默认关。
    pub bash: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SettingsUpdateResponse {
    pub currency: String,
    #[serde(rename = "exec")]
    pub exec: ExecSettingsView,
}

/// 页面异常 / 事件流重连上报：只记 web 的 ops，不进会话日志；正文限长由载体截。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorReportRequest {
    /// 上报种类；未知值按 page-error 记。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// 上报结果（页面不读内容；ok = 已记下）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorReportResponse {
    pub ok: bool,
}

/// 选模型（连接、模型、想要的档）；只影响下一轮（D26）。
///
/// 三个字段都可选（协议只增）：None = 保持当前值。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct SessionModelSelectResponse {
    pub connection: String,
    pub model: String,
    #[serde(rename = "effortWanted")]
    pub effort_wanted: String,
    /// 这一轮实际会发的档（没有想要的档时是最近档；页面据此显示并告诉用户）。
    #[serde(rename = "effortActual")]
    pub effort_actual: String,
}

/// 确认卡/结果卡上的一个动作：确认一项、暂不，或者门判定的通过/需修正。
///
/// 卡片由 propose_action 或 relay 的 call id 定位；option 是选项 id（暂不、判定不需要）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CardActRequest {
    #[serde(rename = "callId")]
    pub call_id: String,
    pub verb: CardVerb,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub option: Option<String>,
    /// 门「需修正」时用户原话（原样存）；其他动作不用。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// T054：结果卡上的报告 id（接受时用卡上那份，不再拿最近报告猜）。
    #[serde(default, rename = "reportId", skip_serializing_if = "Option::is_none")]
    pub report_id: Option<String>,
}

/// 确认卡/结果卡上的按钮；按钮文案和动作由程序定，模型给不了。
///
/// T053：门结果卡用 Pass/NeedsFix 两个程序按钮（needs-fix 带用户原话 note）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CardVerb {
    Confirm,
    Dismiss,
    Pass,
    NeedsFix,
}

/// 动作的受理结果：已经记过的决定原样带回（重复点击不重复执行）；没有动作时为 None。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct CardActResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<session::CardDecision>,
}

/// T053：门判定的结论（协议层，映射到任务库的 GateConclusion）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum GateJudgeConclusion {
    Pass,
    NeedsFix,
}

/// T053：用户在一个门结果卡上判定（内部操作；Kind 没有判定工具）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct GateJudgeRequest {
    #[serde(rename = "gateTask")]
    pub gate_task: String,
    #[serde(rename = "gateReport")]
    pub gate_report: String,
    pub conclusion: GateJudgeConclusion,
    /// 需修正时的用户原话；空白当没有。原样存，不截断不改写。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// T053：Kind 让一个任务在原要求里接着做（task_continue）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct TaskContinueRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    /// 需要用户原样转交的话（需修正时）；空白当没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// 接着做的受理结果：新 Attempt 编号。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct TaskContinueResponse {
    #[serde(rename = "attemptId")]
    pub attempt_id: u64,
}

/// 一条已记下的判定（给页面/测试看的事实）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct GateVerdictView {
    #[serde(rename = "gateReport")]
    pub gate_report: String,
    #[serde(rename = "candidateReport")]
    pub candidate_report: String,
    #[serde(rename = "requirementVersion")]
    pub requirement_version: u32,
    pub conclusion: GateJudgeConclusion,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// T057：判定者（用户/程序）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judged_by: Option<String>,
    /// T057：判定绑定的程序门命令版本；用户判门为 0。
    #[serde(default)]
    pub commands_version: u32,
    pub at: i64,
}

/// 判定结果；「已经变了」时 changed=true、verdict=None（不记判定）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct GateJudgeResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verdict: Option<GateVerdictView>,
    pub changed: bool,
    pub duplicate: bool,
    pub summary: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub released: Vec<String>,
}

/// 存一份文字附件（data 是 base64 的原始字节）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AttachmentPutRequest {
    pub name: String,
    /// base64 编码的内容；不规范直接拒收（request/invalid）。
    pub data: String,
}

/// 存好后的附件 id（sha256:<64hex>）与字节数。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AttachmentPutResponse {
    pub id: String,
    pub name: String,
    pub bytes: u64,
}

/// 按 id 读回附件。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AttachmentGetRequest {
    pub id: String,
}

/// 读回的附件（文字文件附全文；二进制不进仓库，text 为 None 兼容旧数据）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct AttachmentGetResponse {
    pub name: String,
    pub bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

/// @ 文件候选（query 是 @ 后面那段；空就是列前面的）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS)]
pub struct FileReferencesListRequest {
    #[serde(default)]
    pub query: String,
}

/// 一个候选（相对工作目录的路径 + 内容字节数）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct FileReferenceEntry {
    pub path: String,
    pub bytes: u64,
}

/// @ 文件候选列表。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default, TS)]
pub struct FileReferencesListResponse {
    pub references: Vec<FileReferenceEntry>,
}

/// 编辑连接，只改 endpoint/model（不换 key）。
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize, TS)]
#[ts(optional_fields)]
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionUpdateResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 撤销 key（连接保留，之后签发失败）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionRevokeRequest {
    pub id: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct ConnectionRevokeResponse {
    pub connection: credentials::ConnectionInfo,
}

/// 记忆页读取（13 篇 4.1 的 M02/M05/M11 查询面）。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryReadRequest {
    /// 搜索词；给空就是只读全量视图。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    /// 条目详情：给出时响应里带这一条的修订历史。
    #[serde(default, rename = "itemId", skip_serializing_if = "Option::is_none")]
    pub item_id: Option<String>,
    /// 页面上正显示的小卡 ref：请求按 ref 从库读状态（撤销/已允许/删了）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub refs: Vec<String>,
}

/// 记忆条目（页面/卡片用；字段和库里的视图一一对应）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryItemView {
    pub id: String,
    pub layer: String,
    pub binding: String,
    pub scope: String,
    pub title: String,
    pub summary: String,
    pub body: String,
    #[serde(rename = "sourceKind")]
    pub source_kind: String,
    #[serde(rename = "sourceRef")]
    pub source_ref: String,
    pub status: String,
    pub revision: u64,
    #[serde(rename = "updatedAt")]
    pub updated_at: i64,
    pub pinned: bool,
    #[serde(rename = "validFrom")]
    pub valid_from: i64,
    #[serde(
        rename = "validUntil",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub valid_until: Option<i64>,
    #[serde(
        rename = "invalidReason",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub invalid_reason: Option<String>,
    #[serde(
        rename = "supersededBy",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub superseded_by: Option<String>,
    #[serde(rename = "possiblyStale")]
    pub possibly_stale: bool,
}

/// 一条日记（只读）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryDiaryView {
    pub id: String,
    pub day: String,
    pub text: String,
    #[serde(rename = "sourceRef")]
    pub source_ref: String,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

/// 一条修订（条目详情的历史）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryRevisionView {
    pub revision: u64,
    pub title: String,
    pub body: String,
    pub reason: String,
    /// 这一版是谁改的（user/kind/dream/system）。
    pub actor: String,
    #[serde(
        rename = "dreamRunId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub dream_run_id: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: i64,
}

/// 搜索命中：记忆/日记/对话，带出处。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryHitView {
    /// memory / diary / conversation
    pub kind: String,
    /// 记忆或日记的 id；对话命中是会话 id（seq 指回那一轮）。
    pub id: String,
    pub title: String,
    pub snippet: String,
    #[serde(
        rename = "conversationId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

/// 一张程序卡按 ref 现在的状态（active/accepted/deleted/revoked/failed/running）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryCardStateView {
    pub reference: String,
    pub state: String,
    /// 失败类卡片的一行原因（目前是没跑成的做梦 run.error）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// 做梦的一处改动（明细里能逐处撤）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryDreamChangeView {
    #[serde(rename = "memoryId")]
    pub memory_id: String,
    pub title: String,
    pub action: String,
    pub reason: String,
    #[serde(rename = "beforeRevision")]
    pub before_revision: u64,
    #[serde(rename = "afterRevision")]
    pub after_revision: u64,
    /// 这一处的当前修订还是这次做梦写的那版（用户后来改过就不可撤）。
    pub reversible: bool,
    /// 这一处已经撤销过（整次或单独撤）。
    pub undone: bool,
}

/// 一次做梦的记录（页面明细）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryDreamRunView {
    pub id: String,
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    #[serde(rename = "endedAt")]
    pub ended_at: i64,
    pub outcome: String,
    pub error: String,
    pub model: String,
    #[serde(rename = "reportJson")]
    pub report_json: String,
    pub changes: Vec<MemoryDreamChangeView>,
}

/// 记忆页的做梦一节。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryDreamView {
    pub enabled: bool,
    pub permit: bool,
    #[serde(rename = "intervalHours")]
    pub interval_hours: u64,
    #[serde(rename = "lastSuccessAt")]
    pub last_success_at: i64,
    #[serde(rename = "nextAt", default, skip_serializing_if = "Option::is_none")]
    pub next_at: Option<i64>,
    /// 现在有没有在做梦（页面「正在整理」用）。
    pub running: bool,
    pub runs: Vec<MemoryDreamRunView>,
}

/// 记忆页读取的完整响应。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryReadResponse {
    pub version: u64,
    #[serde(rename = "aboutYou")]
    pub about_you: Vec<MemoryItemView>,
    pub entries: Vec<MemoryItemView>,
    pub notes: Vec<MemoryItemView>,
    pub diary: Vec<MemoryDiaryView>,
    pub quarantine: Vec<MemoryItemView>,
    pub search: Vec<MemoryHitView>,
    pub cards: Vec<MemoryCardStateView>,
    pub dream: MemoryDreamView,
    #[serde(rename = "itemRevisions")]
    pub item_revisions: Vec<MemoryRevisionView>,
}

/// 一次记忆变更（13 篇 4.1：一次只改一个对象/一处动作）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MemoryActionKind {
    /// 更正一条（出新修订）。
    Correct {
        id: String,
        title: String,
        summary: String,
        body: String,
        reason: String,
    },
    /// 删到隔离区（7 天内能恢复）。
    Delete {
        id: String,
    },
    Restore {
        id: String,
    },
    /// 立即彻底删除（留无明文 tombstone）。
    Forget {
        id: String,
    },
    /// 把一条笔记定下来（产生有约束力的条目，原笔记标记）。
    Pin {
        id: String,
    },
    /// 撤销做梦：只给 memoryId 撤一处；都不给撤最近一次成功。
    UndoDream {
        #[serde(rename = "runId", default, skip_serializing_if = "Option::is_none")]
        run_id: Option<String>,
        #[serde(rename = "memoryId", default, skip_serializing_if = "Option::is_none")]
        memory_id: Option<String>,
    },
    SetDreamEnabled {
        enabled: bool,
    },
    SetDreamInterval {
        hours: u64,
    },
    /// 观察许可：允许/暂不。
    SetPermit {
        allow: bool,
    },
    /// 用户说「马上整理一次」。
    DreamNow,
    /// 替换卡：替换 / 两条都留；在库里的卡状态上原子生效一次。
    ResolveReplace {
        card: String,
        replace: bool,
    },
    /// 关于你草稿卡：保存 / 不用；在库里的卡状态上原子生效一次。
    ResolveAboutDraft {
        card: String,
        save: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryActionRequest {
    pub action: MemoryActionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MemoryActionResponse {
    pub version: u64,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ActionDescribeRequest {
    pub action: String,
    #[ts(type = "Record<string, unknown>")]
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ActionFact {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ActionDescribeResponse {
    pub action: String,
    pub facts: Vec<ActionFact>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ActionActRequest {
    pub action: String,
    #[ts(type = "Record<string, unknown>")]
    pub details: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct ActionActResponse {
    pub summary: String,
    #[ts(type = "Record<string, unknown>")]
    pub data: serde_json::Value,
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
/// - 9：status 带会话的 cwd（页面把工作过程里的路径显示成相对路径）；
///   确认卡的 card-act 操作与 card/decision 事件也是只增的，跟着这个版本一起发
/// - 11：send 加 resultCard（结果卡「要求修改」进 Kind 输入框；消息内容块加 result-card，
///   投给模型是一行「用户要求修改这张结果卡：任务 <id>「标题」，第 N 版要求的结果」）
/// - 12：work（工作页和「现在」共用一份工作快照：全部任务事实 + 五种「等你」项；
///   组接受 task-accept-many 与它同批出）。旧页忽略新增方法，旧服务按未知方法拒绝。
///   （旧页不会发 card-act；旧服务读到这个操作按 request/invalid-json 拒绝）。
///   Exa 凭证的四个操作（exa-key、exa-key-save、exa-key-check、exa-key-delete）和
///   exa/no-key、exa/invalid-key 两个错误码同属只增面，也随这个版本发。
///   旧页忽略新增字段，旧服务的 status 没有它时页面照绝对路径显示。
/// - 10：删 connection-verify；连接的 check 改由保存时拉一次厂商列表写
///   （新连接/换 key 才拉；旧页发这个方法按未知方法拒绝）。
/// - 22：记忆替换卡/关于你草稿卡的两种 resolve 动作（ResolveReplace/ResolveAboutDraft）；
///   记忆条目视图加成立/失效/被替换字段。只增，旧页忽略。
/// - 16：记忆页——memory-read（读关于你/条目/日记/隔离区/做梦/搜索/卡状态）、
///   memory-action（更正/删/恢复/彻底删/定下来/撤销做梦/开关/间隔/许可/马上整理）；
///   推送 memory/changed { version }（seq=0 的合成事件，页面收到重读）。只增，旧页忽略。
///
/// 连接上的旧价格乘数后来整条删掉，不升版本：两个请求里的这个字段本来就可省略，旧页再发
/// 由 serde 当未知字段忽略；响应里少了它，旧页读到 undefined 也不会拿去定价。
pub const PROTOCOL_VERSION: u64 = 27;

/// 日历读取：不带 appointmentId 读全部；带 week 时额外给那周的计划点。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarReadRequest {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appointment_id: Option<String>,
    /// 区间起点（毫秒，含）；None = 不带区间事实。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<i64>,
    /// 区间终点（毫秒，含）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarReadResponse {
    pub generated_at: i64,
    pub appointments: Vec<CalendarAppointmentView>,
    /// 区间里的运行块（起止/状态词/task_id/turn；约定状态不限）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ranges: Vec<CalendarRunBlock>,
    /// 区间里的计划块（只在用、不密的规则）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub plans: Vec<CalendarPlanBlock>,
    /// 密规则每天计数（跑了几次/没变几次）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dense: Vec<CalendarDenseDay>,
    /// 每条规则的颜色序号（按约定创建先后，6 色循环）。
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub colors: Vec<CalendarRuleColor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarRunBlock {
    pub id: i64,
    pub appointment_id: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    /// 派出任务的阶段 key；有它时页面用看板同一张词表出词。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
    /// 不是派出任务时的固定词（自办块：已完成/在做）。
    pub status: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarPlanBlock {
    pub appointment_id: String,
    pub title: String,
    pub start: i64,
    pub end: i64,
    pub color: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarDenseDay {
    pub appointment_id: String,
    pub title: String,
    /// 当天本地 0 点（毫秒）。
    pub day: i64,
    pub runs: u32,
    pub unchanged: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarRuleColor {
    pub appointment_id: String,
    pub color: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarAppointmentView {
    pub id: String,
    pub title: String,
    pub condition: String,
    pub rule: String,
    /// 服务翻好的中文触发事实（一次/每天/工作日/…；认不出时是规则原文）。
    pub trigger: String,
    pub timezone: String,
    /// active / paused / ended / cancelled。
    pub status: String,
    pub version: u32,
    /// kind / task。
    pub binding: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bound_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_trigger_at: Option<i64>,
    /// 用户说过的时长（分钟）；没说就没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_minutes: Option<u32>,
    /// 暂停原因（程序写；恢复后清空视图，但版本历史留着）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pause_reason: Option<String>,
    /// 盯变化的来源（url / file / taskFacts）与目标；没盯就没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watch_target: Option<String>,
    /// 循环任务绑的任务与最多轮数；不是循环就没有。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_task_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cycle_max_rounds: Option<u32>,
    /// 发起这条约定的 Kind 轮号；旧记录/迁移可空。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_turn: Option<u64>,
    pub created_at: i64,
    pub updated_at: i64,
    pub runs: Vec<CalendarRunView>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarRunView {
    pub id: i64,
    pub nominal_at: i64,
    pub manual: bool,
    /// due / manual / changed / report。
    pub trigger: String,
    /// queued / sent / skipped / failed。
    pub result: String,
    pub reason: String,
    pub merged_count: u32,
    pub created_at: i64,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sent_turn: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarWeekPoint {
    pub at: i64,
    pub appointment_id: String,
    /// run（过去有结果的运行）/ plan（以后按规则算的计划点）。
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum CalendarActionKind {
    Pause,
    Resume,
    RunNow,
    Cancel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarActionRequest {
    pub appointment_id: String,
    pub action: CalendarActionKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct CalendarActionResponse {
    pub appointment: CalendarAppointmentView,
}

/// 根会话不可用的种类：服务照常起，ready 报出来，方法一律拒绝。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct ContextUsage {
    pub used: u64,
    pub window: u64,
}

/// 宿主机的位置（ready 帧的 host）：页面只用 home 把路径缩写成 ~/…。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct HostLocation {
    /// 宿主进程的 HOME（没有就不给；页面只在有值时缩写）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<String>,
}

/// ready/status 里给页面的宿主信息（协议版本 + 根会话状态 + 上下文占用 + 宿主位置）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
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

/// 监控消耗的范围口径：Today=用户时区当天 0 点起；Last7Days=含今天往前 7 天；Month=当月 1 日起。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "kebab-case")]
pub enum MonitorRange {
    Today,
    Last7Days,
    Month,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MonitorSnapshotRequest {
    pub range: MonitorRange,
    /// IANA 时区名（用户本机）；空/不认识时按 UTC。
    pub timezone: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MonitorTaskRequest {
    #[serde(rename = "taskId")]
    pub task_id: String,
    #[serde(rename = "attemptId", default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<u64>,
}

/// 一条任务/模型/工具/命令的耗用事实。金额没有价时为 None，unpriced=true。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorUsageRow {
    pub key: String,
    pub label: String,
    pub requests: u64,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "cacheReadTokens")]
    pub cache_read_tokens: u64,
    #[serde(rename = "cacheWriteTokens")]
    pub cache_write_tokens: u64,
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub unpriced: bool,
}

/// 任务卡/在跑卡共用的一条任务摘要；项目归属先留 Option（None=个人）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorTaskSummary {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub title: String,
    pub state: String,
    pub model: String,
    #[serde(rename = "projectId", default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    #[serde(rename = "attemptId", default, skip_serializing_if = "Option::is_none")]
    pub attempt_id: Option<u64>,
    pub running: bool,
    #[serde(rename = "startedAt", default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    #[serde(rename = "endedAt", default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    pub requests: u64,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub unpriced: bool,
    #[serde(rename = "toolCalls")]
    pub tool_calls: u64,
    #[serde(rename = "contextUsed")]
    pub context_used: u64,
    #[serde(rename = "contextWindow")]
    pub context_window: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorProject {
    #[serde(rename = "projectId")]
    pub project_id: String,
    pub label: String,
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub unpriced: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorRings {
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub unpriced: bool,
    #[serde(rename = "busyMs")]
    pub busy_ms: u64,
    #[serde(rename = "requestMs")]
    pub request_ms: u64,
    #[serde(rename = "toolMs")]
    pub tool_ms: u64,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorAnomaly {
    pub kind: String,
    #[serde(rename = "taskId", default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    pub detail: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorRunning {
    pub seats: u64,
    pub running: u64,
    pub queue: u64,
    pub anomalous: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tasks: Vec<MonitorTaskSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorDay {
    /// 用户时区下的 YYYY-MM-DD。
    pub day: String,
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub unpriced: u64,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    pub requests: u64,
    #[serde(rename = "toolCalls")]
    pub tool_calls: u64,
    #[serde(rename = "commandCalls")]
    pub command_calls: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorSnapshotResponse {
    pub range: MonitorRange,
    pub timezone: String,
    #[serde(rename = "generatedAt")]
    pub generated_at: i64,
    /// 用户时区下按事件 time 落天的模型/工具/命令事实；范围外的天不出现。
    #[serde(default)]
    pub days: Vec<MonitorDay>,
    #[serde(default)]
    pub tasks: Vec<MonitorTaskSummary>,
    #[serde(default)]
    pub projects: Vec<MonitorProject>,
    #[serde(default)]
    pub models: Vec<MonitorUsageRow>,
    #[serde(default)]
    pub tools: Vec<MonitorUsageRow>,
    #[serde(default)]
    pub commands: Vec<MonitorUsageRow>,
    pub rings: MonitorRings,
    #[serde(default)]
    pub anomalies: Vec<MonitorAnomaly>,
    pub running: MonitorRunning,
    #[serde(default)]
    pub queue: Vec<MonitorTaskSummary>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorTaskInfo {
    #[serde(rename = "taskId")]
    pub task_id: String,
    pub title: String,
    pub state: String,
    pub model: String,
    #[serde(rename = "projectId", default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    pub running: bool,
    #[serde(
        rename = "writeScope",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub write_scope: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorAttempt {
    #[serde(rename = "attemptId")]
    pub attempt_id: u64,
    #[serde(rename = "startedAt")]
    pub started_at: i64,
    #[serde(rename = "endedAt", default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<i64>,
    #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub requests: u64,
    #[serde(rename = "inputTokens")]
    pub input_tokens: u64,
    #[serde(rename = "outputTokens")]
    pub output_tokens: u64,
    #[serde(rename = "costUsd", default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
    pub unpriced: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
pub struct MonitorContextParts {
    #[serde(rename = "fixedBlock")]
    pub fixed_block: u64,
    #[serde(rename = "toolDefinitions")]
    pub tool_definitions: u64,
    #[serde(rename = "taskBookMessages")]
    pub task_book_messages: u64,
    #[serde(rename = "modelReplies")]
    pub model_replies: u64,
    #[serde(rename = "toolResults")]
    pub tool_results: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorTodo {
    pub content: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
pub struct MonitorTaskResponse {
    pub task: MonitorTaskInfo,
    #[serde(default)]
    pub attempts: Vec<MonitorAttempt>,
    #[serde(
        rename = "contextParts",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub context_parts: Option<MonitorContextParts>,
    #[serde(default)]
    pub todo: Vec<MonitorTodo>,
    #[serde(default)]
    pub events: Vec<ServiceEvent>,
}
