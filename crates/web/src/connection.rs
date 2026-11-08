//! WebSocket 连接（T019.E 从 lib.rs 拆出）：线上帧、ready/补齐、live 事件和连接内 ops。

use std::sync::{Arc, Mutex};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::response::Response;
use monitor::{Channel, Monitor, Record, Severity};
use serde::{Deserialize, Serialize};
use service::{
    ContextUsage, FollowRequest, FollowResponse, HostInfo, HostLocation, PROTOCOL_VERSION,
    RootState, ServiceEvent,
};

use crate::routes::{AppState, record_ops, trust_error_response};
use crate::trust::authorize_api_request;

/// 线上帧：ready 必须先于事件；事件帧包着 service 的日志事件（seq/type/data 来自日志事实）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
}

/// AgentEvent 的线上形状（只挑页面要的；只增不改）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    upgrade.on_upgrade(move |socket| ws_session(socket, state, last_seq))
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

/// 一个 WebSocket 连接：ready → 补齐（lastSeq 之后）→ live（seq 去重；Lagged 时重新 follow 补齐）。
async fn ws_session(mut socket: WebSocket, state: Arc<AppState>, last_seq: u64) {
    let _stream_ops = StreamOps::open(&state);
    let info = state.host.info();
    let mut live = state.host.subscribe_events();
    let mut agent_live = state.host.subscribe_agent_events();
    let root_available = info.root == RootState::Ok;
    let follow = if root_available {
        match state.host.follow(FollowRequest { last_seq }).await {
            Ok(follow) => follow,
            Err(error) => {
                let value = serde_json::json!({"type": "error", "message": error.to_string()});
                let _ = send_value(&mut socket, &value).await;
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
        if !send_frame(&mut socket, &frame).await {
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
    loop {
        tokio::select! {
            received = live.recv() => match received {
                Ok(event) => {
                    if event.seq == 0 {
                        // 合成事件（设置变化）：不在日志里，不推进 lastSeq；页面收到后自己重拉设置。
                        if !send_frame(&mut socket, &Frame::Event { event }).await {
                            return;
                        }
                        continue;
                    }
                    if event.seq <= last {
                        continue;
                    }
                    last = event.seq;
                    if !send_frame(&mut socket, &Frame::Event { event }).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    if let Ok(follow) = state.host.follow(FollowRequest { last_seq: last }).await {
                        for event in follow.events {
                            if event.seq <= last {
                                continue;
                            }
                            last = event.seq;
                            if !send_frame(&mut socket, &Frame::Event { event }).await {
                                return;
                            }
                        }
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            },
            agent_received = agent_live.recv() => match agent_received {
                Ok(event) => {
                    let Some(wire) = AgentEventWire::from_event(&event) else {
                        continue;
                    };
                    if !send_frame(&mut socket, &Frame::Agent { event: wire }).await {
                        return;
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
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

#[cfg(test)]
mod t015_5_wire_tests {
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
