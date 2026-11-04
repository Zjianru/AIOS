//! 模型层的稳定错误码与错误类型。

use thiserror::Error;

/// 稳定的 provider 无关错误码（日志、CLI 和重试策略路由都用它）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    Auth,
    Quota,
    RateLimit,
    ContextWindowExceeded,
    InvalidRequest,
    Server,
    Transport,
    Timeout,
    Aborted,
    StreamClosed,
    MalformedResponse,
    EmptyResponse,
    MissingCredential,
    Keyring,
    UnsupportedContent,
    UnsupportedReasoningEffort,
    /// 没归类的 HTTP 状态。
    Http(u16),
}

impl ErrorCode {
    /// 错误码的 wire 文本。
    pub fn as_str(self) -> String {
        match self {
            Self::Http(status) => format!("HTTP_{status}"),
            other => other.name().to_owned(),
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Auth => "AUTH",
            Self::Quota => "QUOTA",
            Self::RateLimit => "RATE_LIMIT",
            Self::ContextWindowExceeded => "CONTEXT_WINDOW_EXCEEDED",
            Self::InvalidRequest => "INVALID_REQUEST",
            Self::Server => "SERVER",
            Self::Transport => "TRANSPORT",
            Self::Timeout => "TIMEOUT",
            Self::Aborted => "ABORTED",
            Self::StreamClosed => "STREAM_CLOSED",
            Self::MalformedResponse => "MALFORMED_RESPONSE",
            Self::EmptyResponse => "EMPTY_RESPONSE",
            Self::MissingCredential => "MISSING_CREDENTIAL",
            Self::Keyring => "KEYRING",
            Self::UnsupportedContent => "UNSUPPORTED_CONTENT",
            Self::UnsupportedReasoningEffort => "UNSUPPORTED_REASONING_EFFORT",
            Self::Http(_) => "HTTP",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.as_str())
    }
}

/// 模型层错误：稳定错误码 + 可读消息。
#[derive(Debug, Error)]
#[error("{code}: {message}")]
pub struct ModelError {
    pub code: ErrorCode,
    pub message: String,
    /// HTTP 状态（拿到才有）。
    pub status: Option<u16>,
    /// 服务端 retry-after（毫秒，拿到才 >0）；重试层优先用它。
    pub retry_after_ms: Option<u64>,
}

impl ModelError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            status: None,
            retry_after_ms: None,
        }
    }
}
