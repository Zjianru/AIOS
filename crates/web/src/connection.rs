//! WebSocket 连接：线上帧、ready/补齐、live 事件和连接内 ops。

use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use monitor::{Channel, Monitor, Record, Severity};
use serde::{Deserialize, Serialize};
use service::{
    ContextUsage, FollowRequest, FollowResponse, HostInfo, HostLocation, PROTOCOL_VERSION,
    RootHost, RootState, ServiceError, ServiceEvent, ServiceHost, TaskLive, TaskStream,
};
use tokio::sync::broadcast;
use ts_rs::TS;

use crate::routes::{AppState, record_ops, trust_error_response};
use crate::trust::authorize_api_request;

/// 线上帧：ready 必须先于事件；事件帧包着 service 的日志事件（seq/type/data 来自日志事实）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Frame {
    /// 连接建立后的第一帧（DSH 的 {type:'ready',...}）；T007.2 起带协议版本、根会话状态、上下文占用。
    Ready {
        #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        #[serde(rename = "protocolVersion")]
        protocol_version: u64,
        root: RootState,
        context: ContextUsage,
        /// T009.5：host: {home}（页面用来缩写路径）。
        host: HostLocation,
    },
    /// 日志事件帧（DSH 的 {type:'event', event: SessionWireEvent}）。
    Event { event: ServiceEvent },
    /// T007.4：不落日志的 AgentEvent（流式文本/用量/重试提示等），页面单独消费。
    Agent { event: AgentEventWire },
    /// T058：任务事实变了（守卫门判定/命令版本、接受），页面重读交付记录。
    TaskChanged {
        #[serde(rename = "taskId")]
        task_id: String,
    },
    /// 监控订阅里一个任务会话的 live 事件（带任务/Attempt 标签）。
    TaskEvent {
        #[serde(rename = "taskId")]
        task_id: String,
        #[serde(rename = "attemptId")]
        attempt_id: u64,
        event: ServiceEvent,
    },
}

/// AgentEvent 的线上形状（只挑页面要的；只增不改）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum AgentEventWire {
    TextDelta {
        text: String,
    },
    /// T015.5：工具调用参数的增量（不落日志；页面拿 canvas 的 title 画「正在画」）。
    ToolCallDelta {
        #[serde(rename = "callId")]
        call_id: String,
        name: String,
        #[serde(rename = "argumentsDelta")]
        arguments_delta: String,
    },
    Usage {
        #[serde(
            rename = "inputTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        input_tokens: Option<u64>,
        #[serde(
            rename = "outputTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        output_tokens: Option<u64>,
        #[serde(
            rename = "cacheReadTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        cache_read_tokens: Option<u64>,
        #[serde(
            rename = "cacheWriteTokens",
            default,
            skip_serializing_if = "Option::is_none"
        )]
        cache_write_tokens: Option<u64>,
    },
    Retrying {
        attempt: u32,
        #[serde(rename = "maxAttempts")]
        max_attempts: u32,
        #[serde(rename = "delayMs")]
        delay_ms: u64,
        code: String,
    },
}

impl AgentEventWire {
    pub fn from_event(event: &agent::AgentEvent) -> Option<Self> {
        match event {
            agent::AgentEvent::AssistantTextDelta { text } => {
                Some(Self::TextDelta { text: text.clone() })
            }
            agent::AgentEvent::ToolCallDelta {
                call_id,
                name,
                arguments_delta,
            } => Some(Self::ToolCallDelta {
                call_id: call_id.clone(),
                name: name.clone(),
                arguments_delta: arguments_delta.clone(),
            }),
            agent::AgentEvent::UsageReported { usage } => Some(Self::Usage {
                input_tokens: usage.input_tokens,
                output_tokens: usage.output_tokens,
                cache_read_tokens: usage.cache_read_input_tokens,
                cache_write_tokens: usage.cache_creation_input_tokens,
            }),
            agent::AgentEvent::Retrying {
                attempt,
                max_attempts,
                delay_ms,
                code,
            } => Some(Self::Retrying {
                attempt: *attempt,
                max_attempts: *max_attempts,
                delay_ms: *delay_ms,
                code: code.clone(),
            }),
            _ => None,
        }
    }
}

/// 打开连接时的帧序列：ready 第一帧（带宿主信息），然后按 lastSeq 补齐的事件（重连带序号）。
pub fn opening_frames(info: &HostInfo, follow: FollowResponse) -> Vec<Frame> {
    let mut frames = Vec::with_capacity(follow.events.len() + 1);
    frames.push(Frame::Ready {
        session_id: info.session_id.clone(),
        protocol_version: PROTOCOL_VERSION,
        root: info.root.clone(),
        context: info.context,
        host: info.host.clone(),
    });
    frames.extend(
        follow
            .events
            .into_iter()
            .map(|event| Frame::Event { event }),
    );
    frames
}

#[derive(Debug, Deserialize)]
pub(crate) struct WsQuery {
    #[serde(default, rename = "lastSeq")]
    last_seq: Option<u64>,
    /// 只读任务对话：连这个任务当前 Attempt 的会话；省略时连根会话（旧页面行为不变）。
    #[serde(default)]
    task: Option<String>,
    /// 监控订阅：monitor=1 时连所有在跑任务的事件；旧连接不变。
    #[serde(default)]
    monitor: Option<String>,
}

/// T011.4：事件流的连上/断开记录（reason 在 Drop 兜底写）。
pub(crate) struct StreamOps {
    monitor: Option<Monitor>,
    session_id: Option<String>,
    reason: Mutex<&'static str>,
}

impl StreamOps {
    pub(crate) fn open(state: &AppState) -> Self {
        let session_id = state.host.info().session_id;
        record_ops(
            state,
            "stream-open",
            Severity::Info,
            vec![("reason".to_owned(), serde_json::json!("requested"))],
        );
        Self {
            monitor: state.monitor.clone(),
            session_id,
            reason: Mutex::new("closed"),
        }
    }
}

impl Drop for StreamOps {
    fn drop(&mut self) {
        let Some(monitor) = &self.monitor else {
            return;
        };
        let mut map = serde_json::Map::new();
        map.insert("telemetry.op".to_owned(), serde_json::json!("stream-close"));
        if let Some(session_id) = &self.session_id {
            map.insert("session.id".to_owned(), serde_json::json!(session_id));
        }
        if let Ok(reason) = self.reason.lock() {
            map.insert("reason".to_owned(), serde_json::json!(*reason));
        }
        let _ = monitor.record(Record::new(
            Channel::Ops,
            Severity::Info,
            map,
            serde_json::json!({}),
        ));
    }
}

/// GET /ws：先过信任栅栏，再升级 WebSocket；连接内先发 ready 再发事件。
pub(crate) async fn ws_handler(
    State(state): State<Arc<AppState>>,
    Query(query): Query<WsQuery>,
    headers: HeaderMap,
    uri: axum::http::Uri,
    upgrade: WebSocketUpgrade,
) -> Response {
    if let Err(error) = authorize_api_request(&state.trust, &headers, &uri) {
        return trust_error_response(error);
    }
    let last_seq = query.last_seq.unwrap_or(0);
    let task = query.task;
    let monitor = query
        .monitor
        .as_deref()
        .is_some_and(|value| value == "1" || value == "true");
    upgrade.on_upgrade(move |socket| ws_session(socket, state, last_seq, task, monitor))
}

async fn send_value(socket: &mut WebSocket, value: &serde_json::Value) -> bool {
    socket
        .send(Message::Text(value.to_string().into()))
        .await
        .is_ok()
}

async fn send_frame(socket: &mut WebSocket, frame: &Frame) -> bool {
    match serde_json::to_value(frame) {
        Ok(value) => send_value(socket, &value).await,
        Err(_) => false,
    }
}

/// 一个 WebSocket 连接：带 task 时进任务对话，否则是根会话（旧页面行为不变）。
async fn ws_session(
    mut socket: WebSocket,
    state: Arc<AppState>,
    last_seq: u64,
    task: Option<String>,
    monitor: bool,
) {
    let _stream_ops = StreamOps::open(&state);
    if monitor {
        ws_monitor_session(&mut socket, &state, last_seq).await;
        return;
    }
    match task {
        Some(task_id) => ws_task_session(&mut socket, &state, &task_id, last_seq).await,
        None => ws_root_session(&mut socket, &state, last_seq).await,
    }
}

async fn send_error(socket: &mut WebSocket, message: &str) -> bool {
    send_value(
        socket,
        &serde_json::json!({"type": "error", "message": message}),
    )
    .await
}

/// 监控连接：ready 后补各在跑任务的 backlog，再转发 TaskEvent；掉帧发 task-changed 让页面重读。
async fn ws_monitor_session(socket: &mut WebSocket, state: &AppState, last_seq: u64) {
    let info = state.host.info();
    if !send_frame(
        socket,
        &Frame::Ready {
            session_id: info.session_id.clone(),
            protocol_version: info.protocol_version,
            root: info.root.clone(),
            context: info.context,
            host: info.host.clone(),
        },
    )
    .await
    {
        return;
    }
    let Some(runtime) = state.host.task_runtime() else {
        return;
    };
    let live = match runtime.monitor_live(last_seq).await {
        Ok(live) => live,
        Err(error) => {
            let _ = send_error(socket, &error.to_string()).await;
            return;
        }
    };
    let known_tasks: Vec<String> = live.tasks.iter().map(|task| task.task_id.clone()).collect();
    for task in &live.tasks {
        for event in &task.follow.events {
            if !send_frame(
                socket,
                &Frame::TaskEvent {
                    task_id: task.task_id.clone(),
                    attempt_id: task.attempt_id,
                    event: event.clone(),
                },
            )
            .await
            {
                return;
            }
        }
    }
    enum MonitorMsg {
        Event {
            task_id: String,
            attempt_id: u64,
            event: ServiceEvent,
        },
        Changed(String),
    }
    let (sender, mut receiver) = tokio::sync::mpsc::channel::<MonitorMsg>(256);
    for mut task in live.tasks {
        let sender = sender.clone();
        tokio::spawn(async move {
            loop {
                match task.events.recv().await {
                    Ok(event) => {
                        if sender
                            .send(MonitorMsg::Event {
                                task_id: task.task_id.clone(),
                                attempt_id: task.attempt_id,
                                event,
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        if sender
                            .send(MonitorMsg::Changed(task.task_id.clone()))
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => return,
                }
            }
        });
    }
    let mut changed = live.changed;
    loop {
        tokio::select! {
            message = receiver.recv() => match message {
                Some(MonitorMsg::Event { task_id, attempt_id, event }) => {
                    if !send_frame(socket, &Frame::TaskEvent { task_id, attempt_id, event }).await { return; }
                }
                Some(MonitorMsg::Changed(task_id)) => {
                    if !send_frame(socket, &Frame::TaskChanged { task_id }).await { return; }
                }
                None => return,
            },
            message = changed.recv() => match message {
                Ok(task_id) => {
                    if !send_frame(socket, &Frame::TaskChanged { task_id }).await { return; }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    // 任务事实变化也掉了：让页面按已知任务重读快照。
                    if known_tasks.is_empty() {
                        if !send_frame(socket, &Frame::TaskChanged { task_id: String::new() }).await { return; }
                    } else {
                        for task_id in &known_tasks {
                            if !send_frame(socket, &Frame::TaskChanged { task_id: task_id.clone() }).await { return; }
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(message)) if !matches!(message, Message::Close(_)) => continue,
                _ => return,
            },
        }
    }
}

/// 任务对话的连接：同样是 ready → 补齐 → live；live 没接上时只补齐，不出 live 帧。
async fn ws_task_session(socket: &mut WebSocket, state: &AppState, task_id: &str, last_seq: u64) {
    let stream = match state.host.task_stream(task_id, last_seq).await {
        Ok(stream) => stream,
        Err(error) => {
            let _ = send_error(socket, &error.to_string()).await;
            return;
        }
    };
    let mut info = state.host.info();
    info.session_id = Some(stream.session_id.clone());
    let TaskStream {
        session_id: _,
        follow,
        live,
        mut changed,
    } = stream;
    let last = last_seq.max(follow.last_seq);
    for frame in opening_frames(&info, follow) {
        if !send_frame(socket, &frame).await {
            return;
        }
    }
    let Some(live) = live else {
        // 任务没在跑：只补齐；任务事实变化仍可推送，连接留到页面离开。
        loop {
            tokio::select! {
                changed_message = changed.recv() => match changed_message {
                    Ok(task_id) => {
                        if !send_frame(socket, &Frame::TaskChanged { task_id }).await {
                            return;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => return,
                },
                incoming = socket.recv() => match incoming {
                    Some(Ok(message)) if !matches!(message, Message::Close(_)) => continue,
                    _ => return,
                },
            }
        }
    };
    let TaskLive {
        host,
        events,
        agent_events,
    } = live;
    let source = FollowSource::Task(&host);
    live_loop(socket, &source, events, agent_events, Some(changed), last).await;
}

/// 根会话的连接：ready → 补齐（lastSeq 之后）→ live（seq 去重；Lagged 时重新 follow 补齐）。
async fn ws_root_session(socket: &mut WebSocket, state: &AppState, last_seq: u64) {
    let info = state.host.info();
    let live = state.host.subscribe_events();
    let agent_live = state.host.subscribe_agent_events();
    // 先订阅任务事实变化再补齐/发 ready，避免连接建立窗口里的事件丢掉。
    let changed = state.host.subscribe_task_changed();
    let root_available = info.root == RootState::Ok;
    let follow = if root_available {
        match state.host.follow(FollowRequest { last_seq }).await {
            Ok(follow) => follow,
            Err(error) => {
                let _ = send_error(socket, &error.to_string()).await;
                return;
            }
        }
    } else {
        FollowResponse {
            events: Vec::new(),
            last_seq,
        }
    };
    let mut last = last_seq;
    if let Some(seq) = follow.events.last().map(|event| event.seq) {
        last = last.max(seq);
    }
    for frame in opening_frames(&info, follow) {
        if !send_frame(socket, &frame).await {
            return;
        }
    }
    if !root_available {
        // T007.2：根会话不可用时页面根据 ready 显示说明页；连接保持到关闭/重连，
        // 修好后页面调 recheck 再重连。
        while let Some(Ok(message)) = socket.recv().await {
            if matches!(message, Message::Close(_)) {
                return;
            }
        }
        return;
    }
    let source = FollowSource::Root(state.host.as_ref());
    live_loop(socket, &source, live, agent_live, changed, last).await;
}

/// T058：任务事实变化接收；根会话没有这个通道，用永远 pending 的分支。
async fn recv_changed(
    changed: &mut Option<broadcast::Receiver<String>>,
) -> Result<String, broadcast::error::RecvError> {
    match changed {
        Some(receiver) => receiver.recv().await,
        None => std::future::pending().await,
    }
}

/// 已补齐之后的 live 帧：日志事件按 seq 去重，流式文字照发；Lagged 时重新 follow。
async fn live_loop(
    socket: &mut WebSocket,
    source: &FollowSource<'_>,
    mut live: broadcast::Receiver<ServiceEvent>,
    mut agent_live: broadcast::Receiver<agent::AgentEvent>,
    mut changed: Option<broadcast::Receiver<String>>,
    mut last: u64,
) {
    loop {
        tokio::select! {
            received = live.recv() => match received {
                Ok(event) => {
                    if event.seq == 0 {
                        // 合成事件（设置变化）：不在日志里，不推进 lastSeq；页面收到后自己重拉设置。
                        if !send_frame(socket, &Frame::Event { event }).await {
                            return;
                        }
                        continue;
                    }
                    if event.seq <= last {
                        continue;
                    }
                    last = event.seq;
                    if !send_frame(socket, &Frame::Event { event }).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => {
                    if let Ok(follow) = source.follow(last).await {
                        for event in follow.events {
                            if event.seq <= last {
                                continue;
                            }
                            last = event.seq;
                            if !send_frame(socket, &Frame::Event { event }).await {
                                return;
                            }
                        }
                    }
                }
                Err(broadcast::error::RecvError::Closed) => return,
            },
            agent_received = agent_live.recv() => match agent_received {
                Ok(event) => {
                    let Some(wire) = AgentEventWire::from_event(&event) else {
                        continue;
                    };
                    if !send_frame(socket, &Frame::Agent { event: wire }).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            changed_message = recv_changed(&mut changed) => match changed_message {
                Ok(task_id) => {
                    if !send_frame(socket, &Frame::TaskChanged { task_id }).await {
                        return;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = socket.recv() => match incoming {
                Some(Ok(message)) => {
                    if matches!(message, Message::Close(_)) {
                        return;
                    }
                    continue;
                }
                _ => return,
            },
        }
    }
}

/// live 循环的补齐来源：根会话或任务 Attempt。Lagged 时用它重新 follow。
enum FollowSource<'a> {
    Root(&'a RootHost),
    Task(&'a ServiceHost),
}

impl FollowSource<'_> {
    async fn follow(&self, last: u64) -> Result<FollowResponse, ServiceError> {
        let request = FollowRequest { last_seq: last };
        match self {
            Self::Root(host) => host.follow(request).await,
            Self::Task(host) => host.follow(request).await,
        }
    }
}

#[cfg(test)]
mod wire_protocol_tests {
    use super::*;

    /// T015.5（清单 9）：tool-call-delta 的线上帧只增不改——tag 与字段名照票面。
    #[test]
    fn tool_call_delta_wire_frame_shape() {
        let wire = AgentEventWire::from_event(&agent::AgentEvent::ToolCallDelta {
            call_id: "call_1".to_owned(),
            name: "canvas".to_owned(),
            arguments_delta: "{\"title\":\"".to_owned(),
        })
        .expect("tool-call-delta 要发到页面");
        assert_eq!(
            serde_json::to_value(&wire).unwrap(),
            serde_json::json!({
                "type": "tool-call-delta",
                "callId": "call_1",
                "name": "canvas",
                "argumentsDelta": "{\"title\":\"",
            })
        );
    }
}
