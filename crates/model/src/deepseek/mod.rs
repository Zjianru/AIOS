//! DeepSeek Messages 适配器：HTTP 传输、SSE 解码和流事件翻译。

mod config;
mod serialize;
mod sse;
mod translate;
mod transport;

use std::collections::VecDeque;

pub use config::{DEFAULT_BASE_URL, DEFAULT_MAX_TOKENS, DeepSeekConfig};

use crate::{ErrorCode, Message, ModelError, StreamChunk};
use sse::SseParser;
use translate::Translator;

use config::STREAM_IDLE_TIMEOUT;

/// DeepSeek 的 Messages 适配器。
#[derive(Debug, Clone)]
pub struct DeepSeek {
    http: reqwest::Client,
    config: DeepSeekConfig,
    url: reqwest::Url,
}

impl DeepSeek {
    /// 校验配置并建立 HTTP 客户端；缺 key 或非法端点在发请求之前就报错。
    pub fn new(config: DeepSeekConfig) -> Result<Self, ModelError> {
        if config.api_key.trim().is_empty() {
            return Err(ModelError::new(
                ErrorCode::MissingCredential,
                "DEEPSEEK_API_KEY 为空",
            ));
        }
        let url = messages_url(&config.base_url)?;
        let http = reqwest::Client::builder()
            .build()
            .map_err(transport::transport_error)?;
        Ok(Self { http, config, url })
    }

    pub fn config(&self) -> &DeepSeekConfig {
        &self.config
    }

    /// 发一次流式请求。消息按 Messages 协议序列化（system 单独成字段）。
    pub async fn stream(&self, messages: &[Message]) -> Result<ChunkStream, ModelError> {
        let body = serialize::request_body(&self.config, messages)?;
        let response = self
            .http
            .post(self.url.clone())
            .header("content-type", "application/json")
            .header("accept", "text/event-stream")
            .header("x-api-key", &self.config.api_key)
            .header("anthropic-version", "2023-06-01")
            .body(body)
            .send()
            .await
            .map_err(transport::transport_error)?;
        let status = response.status();
        if !status.is_success() {
            let text = response.text().await.unwrap_or_default();
            let raw = serde_json::from_str(&text).unwrap_or(serde_json::Value::Null);
            return Err(transport::provider_error(&raw, Some(status.as_u16())));
        }
        Ok(ChunkStream::new(response))
    }
}

/// 把 base URL 规范化成 Messages 端点：`.../anthropic` -> `.../anthropic/v1/messages`。
fn messages_url(base_url: &str) -> Result<reqwest::Url, ModelError> {
    let trimmed = base_url.trim_end_matches('/');
    let root = if trimmed.ends_with("/v1") {
        trimmed.to_owned()
    } else {
        format!("{trimmed}/v1")
    };
    reqwest::Url::parse(&format!("{root}/messages")).map_err(|error| {
        ModelError::new(
            ErrorCode::InvalidRequest,
            format!("DeepSeek base URL 非法: {error}"),
        )
    })
}

/// 一次请求的流式响应，按块吐出 provider 无关事件。
pub struct ChunkStream {
    response: reqwest::Response,
    parser: SseParser,
    translator: Translator,
    pending: VecDeque<StreamChunk>,
    done: bool,
}

impl ChunkStream {
    fn new(response: reqwest::Response) -> Self {
        Self {
            response,
            parser: SseParser::default(),
            translator: Translator::new(),
            pending: VecDeque::new(),
            done: false,
        }
    }

    /// 取下一个流事件；流正常结束时返回 `None`。
    pub async fn next(&mut self) -> Option<Result<StreamChunk, ModelError>> {
        loop {
            if let Some(chunk) = self.pending.pop_front() {
                return Some(Ok(chunk));
            }
            if self.done {
                return None;
            }
            if let Err(error) = self.pump_frames() {
                self.done = true;
                return Some(Err(error));
            }
            if let Some(chunk) = self.pending.pop_front() {
                return Some(Ok(chunk));
            }
            if self.done {
                return None;
            }
            match tokio::time::timeout(STREAM_IDLE_TIMEOUT, self.response.chunk()).await {
                Err(_) => {
                    self.done = true;
                    return Some(Err(ModelError::new(
                        ErrorCode::Timeout,
                        "DeepSeek Messages 流空闲超时（300 秒）",
                    )));
                }
                Ok(Err(error)) => {
                    self.done = true;
                    return Some(Err(transport::transport_error(error)));
                }
                Ok(Ok(Some(bytes))) => self.parser.push(&bytes),
                Ok(Ok(None)) => {
                    self.done = true;
                    if !self.translator.is_finished() {
                        return Some(Err(ModelError::new(
                            ErrorCode::StreamClosed,
                            "DeepSeek Messages 在 message_stop 之前关闭了流",
                        )));
                    }
                    return None;
                }
            }
        }
    }

    fn pump_frames(&mut self) -> Result<(), ModelError> {
        loop {
            let Some(frame) = self.parser.next_frame()? else {
                return Ok(());
            };
            let Some(data) = sse::frame_data(&frame)? else {
                continue;
            };
            let event: serde_json::Value = serde_json::from_str(&data).map_err(|_| {
                ModelError::new(
                    ErrorCode::MalformedResponse,
                    "DeepSeek Messages 的 SSE 事件不是合法 JSON",
                )
            })?;
            let chunks = self.translator.push(event)?;
            self.pending.extend(chunks);
            if self.translator.is_finished() {
                self.done = true;
                return Ok(());
            }
        }
    }
}
