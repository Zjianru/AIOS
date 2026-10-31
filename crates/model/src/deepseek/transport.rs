//! HTTP 与 in-band 错误的归类。

use serde_json::Value;

use crate::{ErrorCode, ModelError};

/// 把 HTTP 状态和响应体里的错误归类成稳定的 provider 无关错误。
pub(super) fn provider_error(raw: &Value, status: Option<u16>) -> ModelError {
    let envelope = raw.as_object();
    let error = envelope
        .and_then(|object| object.get("error"))
        .and_then(Value::as_object);
    let message = error
        .and_then(|object| object.get("message"))
        .and_then(Value::as_str)
        .map(str::to_owned)
        .unwrap_or_else(|| {
            let detail = status.map_or_else(|| "stream error".to_owned(), |code| code.to_string());
            format!("DeepSeek Messages 请求失败 ({detail})")
        });
    let error_type = error
        .and_then(|object| object.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let code = match status {
        Some(401 | 403) => ErrorCode::Auth,
        Some(402) => ErrorCode::Quota,
        Some(429) => ErrorCode::RateLimit,
        Some(400 | 413) => ErrorCode::InvalidRequest,
        Some(code) if code >= 500 => ErrorCode::Server,
        Some(code) => ErrorCode::Http(code),
        None => match error_type {
            "authentication_error" | "permission_error" => ErrorCode::Auth,
            "rate_limit_error" => ErrorCode::RateLimit,
            "invalid_request_error" => ErrorCode::InvalidRequest,
            "api_error" | "overloaded_error" => ErrorCode::Server,
            _ => ErrorCode::Server,
        },
    };
    ModelError::new(code, message)
}

/// 把 reqwest 的传输错误归类。
pub(super) fn transport_error(error: reqwest::Error) -> ModelError {
    let code = if error.is_timeout() {
        ErrorCode::Timeout
    } else {
        ErrorCode::Transport
    };
    ModelError::new(code, format!("DeepSeek Messages 传输失败: {error}"))
}
