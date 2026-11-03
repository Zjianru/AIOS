//! T006.16a：与载体无关的服务宿主（接缝 3）——调用对象 + 事件流；Web 适配器在 T006.17。
//!
//! DSH 对照：packages/api/session-controller/src/index.ts 的 Host 方法面（prompt/cancel/events/state
//! 等；见 T006/evidence/T006.16a-dsh-compare.log 的引用校正）。本票只做「送调用/收事件」的接缝与
//! 第一版方法/事件形状，不起 HTTP/WS（T006.17），不接页面（T008）。
//!
//! 对象固定 Kind 根会话：根会话实体在 T006.16b 落，本票宿主绑定一个会话、Object 用 session id。

mod root;

use std::sync::Mutex;

use agent::{Agent, SendAdmission};
use model::UserMessage;
pub use root::{
    ROOT_STATE_FILE, RootError, RootSession, RootSessionOptions, RootStatus, open_root_session,
};
use serde::{Deserialize, Serialize};
use tokio::sync::broadcast;

/// 服务事件流容量（订阅者落后时用 Lagged；重新跟随在 T006.16b）。
const SERVICE_EVENT_CHANNEL_CAPACITY: usize = 1024;

/// 服务对象：固定 Kind 根会话（16b 换成根会话实体）；第一版用会话 id 表达。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
}

/// 发消息（DSH session-controller 的 prompt）。requestId 是幂等键：同一 id 只进一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SendResponse {
    #[serde(rename = "requestId")]
    pub request_id: String,
    pub accepted: bool,
    pub duplicate: bool,
}

/// 取消当前轮（不丢 pending inbox，照 T006.2a 的 cancel）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CancelRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CancelResponse {
    pub cancelled: bool,
}

/// 一条答案（接 T006.13 的 {id, selected[], custom?}）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServiceAnswer {
    #[serde(rename = "questionId")]
    pub question_id: String,
    pub selected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom: Option<String>,
}

/// 回答整批（一次调用覆盖一批问题；逐题走 Agent::answer，全部被接受才算成功）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnswerRequest {
    pub answers: Vec<ServiceAnswer>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactCounts {
    #[serde(rename = "shadowedItems")]
    pub shadowed_items: usize,
    #[serde(rename = "shadowedTokens")]
    pub shadowed_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompactResponse {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compacted: Option<CompactCounts>,
}

/// 被动状态（DSH state 的第一版子集）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct StatusRequest {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub object: Object,
    pub running: bool,
    #[serde(rename = "pendingQuestions")]
    pub pending_questions: usize,
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
    /// 页的末端（含）；省略 = 当前日志末端。
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EventsResponse {
    /// 这一页事件（按日志顺序；页首通常是 turn/start，除非预算或日志耗尽先到）。
    pub events: Vec<ServiceEvent>,
    /// 还有更早的事件（下一轮用 beforeSeq = 页首 seq - 1 往回翻）。
    #[serde(rename = "hasMore")]
    pub has_more: bool,
}

/// 服务事件：严格来自会话日志事实（seq 连续、type 是日志事件类型、data 是事件字段）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServiceEvent {
    pub seq: u64,
    #[serde(rename = "type")]
    pub event_type: String,
    pub data: serde_json::Value,
}

/// 服务错误（协议层错误码的第一版；Web 适配器在 17 映射成线上错误）。
#[derive(Debug, thiserror::Error)]
pub enum ServiceError {
    #[error(transparent)]
    Agent(#[from] agent::AgentError),
    #[error("回答被拒绝的问题：{0}")]
    AnswerRejected(String),
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
    /// 骨架先只落协议方法；撤销语义在红测试后接 T006.13 的 Agent::cancel_question。
    pub async fn dismiss_questions(
        &self,
        _request: DismissQuestionsRequest,
    ) -> Result<DismissQuestionsResponse, ServiceError> {
        Ok(DismissQuestionsResponse { dismissed: false })
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
        if all.is_empty() {
            return Ok(EventsResponse {
                events: Vec::new(),
                has_more: false,
            });
        }
        let end_seq = request
            .before_seq
            .unwrap_or(u64::MAX)
            .min(all.last().map(|event| event.seq).unwrap_or(0));
        let max_messages = request.max_messages.unwrap_or(500).max(1);
        let min_messages = request.min_messages.unwrap_or(50).min(max_messages);
        let min_turns = request.min_turns.unwrap_or(2).max(1);

        let mut selected: Vec<&session::Event> = Vec::new();
        let mut messages = 0_usize;
        let mut turns = 0_usize;
        for event in all.iter().filter(|event| event.seq <= end_seq).rev() {
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

/// 会话日志事件 → 服务事件（data = 日志事件的字段，不含 type 标签）。
fn service_event(event: &session::Event) -> ServiceEvent {
    let mut data = serde_json::to_value(&event.kind).unwrap_or(serde_json::Value::Null);
    if let Some(object) = data.as_object_mut() {
        object.remove("type");
    }
    ServiceEvent {
        seq: event.seq,
        event_type: event.kind.type_name().to_owned(),
        data,
    }
}
