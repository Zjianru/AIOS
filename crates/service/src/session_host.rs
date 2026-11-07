//! 裸宿主：只绑定一个 Agent，把调用翻译成 Agent 操作；不碰 Keyring 和根状态。

use std::sync::Mutex;

use agent::{Agent, SendAdmission};
use model::UserMessage;
use tokio::sync::broadcast;

use crate::SERVICE_EVENT_CHANNEL_CAPACITY;
use crate::error::ServiceError;
use crate::events::{page_events, service_event};
use crate::protocol::{
    AnswerRequest, AnswerResponse, Call, CancelRequest, CancelResponse, CompactCounts,
    CompactRequest, CompactResponse, DeliveryMode, DismissQuestionsRequest,
    DismissQuestionsResponse, EventsRequest, EventsResponse, FollowRequest, FollowResponse,
    HostLocation, Object, RecheckResponse, Response, RootState, SendRequest, SendResponse,
    ServiceEvent, StatusRequest, StatusResponse,
};

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
            | Call::ConnectionQuota(_)
            | Call::SettingsUpdate(_)
            | Call::AttachmentPut(_)
            | Call::AttachmentGet(_)
            | Call::FileReferencesList(_) => Err(ServiceError::SettingsOnBareHost),
        }
    }

    /// 发消息：requestId 幂等（DSH prompt 的 requestId；判定在 Agent 的 durable rpcId 上：
    /// 同一 id 已进 inbox 或已落日志就不再插入，重启后仍然有效，T006.16b）。
    pub async fn send(&self, request: SendRequest) -> Result<SendResponse, ServiceError> {
        let message = UserMessage::text(request.content);
        self.send_message(&request.request_id, message, request.delivery)
            .await
    }

    /// T013.4：发送一条已经拼好的块状用户消息（RootHost 解析附件后走这里）。
    pub async fn send_message(
        &self,
        request_id: &str,
        message: UserMessage,
        delivery: DeliveryMode,
    ) -> Result<SendResponse, ServiceError> {
        // 串行受理：并发同 id 时第一个进，其余在 Agent 的 durable 判重里变成 Duplicate。
        let _serialized = self.send_lock.lock().await;
        let admission = self
            .agent
            .send_with_request_id(request_id, message, delivery.into())
            .await?;
        Ok(SendResponse {
            request_id: request_id.to_owned(),
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
