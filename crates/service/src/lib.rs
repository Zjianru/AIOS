//! T006.16a：与载体无关的服务宿主（接缝 3）——调用对象 + 事件流；Web 适配器在 T006.17。
//!
//! DSH 对照：packages/api/session-controller/src/index.ts 的 Host 方法面（prompt/cancel/events/state
//! 等；见 T006/evidence/T006.16a-dsh-compare.log 的引用校正）。本票只做「送调用/收事件」的接缝与
//! 第一版方法/事件形状，不起 HTTP/WS（T006.17），不接页面（T008）。
//!
//! 对象固定 Kind 根会话：根会话实体在 T006.16b 落，本票宿主绑定一个会话、Object 用 session id。

use std::collections::HashMap;
use std::sync::Mutex;

use agent::Agent;
use model::UserMessage;
use serde::{Deserialize, Serialize};
use tokio::sync::{broadcast, oneshot};

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
    Compact(CompactRequest),
    Status(StatusRequest),
}

/// 调用的结果（与 Call 一一对应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "method", rename_all = "kebab-case")]
pub enum Response {
    Send(SendResponse),
    Cancel(CancelResponse),
    Answer(AnswerResponse),
    Compact(CompactResponse),
    Status(StatusResponse),
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
    #[error("发送失败：{0}")]
    SendFailed(String),
}

/// 幂等表：同一 requestId 的并发/重试共享一次受理。
enum SendEntry {
    InFlight(Vec<oneshot::Sender<Result<SendResponse, String>>>),
    Done(SendResponse),
}

/// 与载体无关的服务宿主：绑定一个 Agent/会话，调用经 dispatch 进入，事件从广播流出。
pub struct ServiceHost {
    agent: Agent,
    events: broadcast::Sender<ServiceEvent>,
    /// 同一 requestId 只进一次：并发时后到者等同一次的结果。
    sent: tokio::sync::Mutex<HashMap<String, SendEntry>>,
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
            sent: tokio::sync::Mutex::new(HashMap::new()),
            forward: Mutex::new(Some(forward)),
        }
    }

    /// 服务对象（第一版：会话 id；16b 换成 Kind 根会话实体）。
    pub fn object(&self) -> Object {
        Object {
            session_id: self.agent.id().as_str().to_owned(),
        }
    }

    /// 事件订阅端（载体每连接一个）。
    pub fn events(&self) -> broadcast::Receiver<ServiceEvent> {
        self.events.subscribe()
    }

    /// 唯一的调用入口：载体把线上请求解码成 Call，结果编码回 Response。
    pub async fn dispatch(&self, call: Call) -> Result<Response, ServiceError> {
        match call {
            Call::Send(request) => self.send(request).await.map(Response::Send),
            Call::Cancel(request) => self.cancel(request).await.map(Response::Cancel),
            Call::Answer(request) => self.answer(request).await.map(Response::Answer),
            Call::Compact(request) => self.compact(request).await.map(Response::Compact),
            Call::Status(request) => self.status(request).await.map(Response::Status),
        }
    }

    /// 发消息：requestId 幂等（DSH prompt 的 requestId：已受理的重试返回原始受理，不重复插入）。
    pub async fn send(&self, request: SendRequest) -> Result<SendResponse, ServiceError> {
        let mut waiter = None;
        {
            let mut sent = self.sent.lock().await;
            match sent.get_mut(&request.request_id) {
                Some(SendEntry::Done(response)) => {
                    return Ok(SendResponse {
                        duplicate: true,
                        ..response.clone()
                    });
                }
                Some(SendEntry::InFlight(waiters)) => {
                    let (tx, rx) = oneshot::channel();
                    waiters.push(tx);
                    waiter = Some(rx);
                }
                None => {
                    sent.insert(request.request_id.clone(), SendEntry::InFlight(Vec::new()));
                }
            }
        }
        if let Some(waiter) = waiter {
            return match waiter.await {
                Ok(Ok(response)) => Ok(SendResponse {
                    duplicate: true,
                    ..response
                }),
                Ok(Err(message)) => Err(ServiceError::SendFailed(message)),
                Err(_) => Err(ServiceError::SendFailed("等待原始受理失败".to_owned())),
            };
        }

        let result = self
            .agent
            .send(UserMessage::text(request.content), request.delivery.into())
            .await;
        let response = SendResponse {
            request_id: request.request_id.clone(),
            accepted: result.is_ok(),
            duplicate: false,
        };
        {
            let mut sent = self.sent.lock().await;
            if let Some(SendEntry::InFlight(waiters)) = sent.remove(&request.request_id) {
                for waiter in waiters {
                    let _ = waiter.send(
                        result
                            .as_ref()
                            .map(|()| response.clone())
                            .map_err(|error| error.to_string()),
                    );
                }
            }
            // 失败不占幂等键：同一个 id 可以重试；成功则归档原始受理。
            if result.is_ok() {
                sent.insert(
                    request.request_id.clone(),
                    SendEntry::Done(response.clone()),
                );
            }
        }
        result.map(|()| response).map_err(ServiceError::Agent)
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
