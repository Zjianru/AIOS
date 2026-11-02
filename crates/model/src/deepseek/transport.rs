//! HTTP 与 in-band 错误的归类。

use serde_json::Value;

use crate::{ErrorCode, ModelError};

/// 把 HTTP 状态和响应体里的错误归类成稳定的 provider 无关错误。
/// retry-after 只认数字秒；HTTP 日期形态暂不支持（重试层回退本地退避，handback 记偏离）。
pub(super) fn provider_error(
    raw: &Value,
    status: Option<u16>,
    headers: Option<&reqwest::header::HeaderMap>,
) -> ModelError {
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
        Some(400 | 413) => {
            if is_context_overflow(&message, error_type) {
                ErrorCode::ContextWindowExceeded
            } else {
                ErrorCode::InvalidRequest
            }
        }
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
    let mut error = ModelError::new(code, message);
    error.status = status;
    error.retry_after_ms = retry_after_ms(headers);
    error
}

/// 上下文溢出识别（照 DSH 的 provider 错误归一）：状态 400/413 且错误类型/文案符合时
/// 归到 `CONTEXT_WINDOW_EXCEEDED`，驱动层据此先压缩再重试一次。
fn is_context_overflow(message: &str, error_type: &str) -> bool {
    if matches!(error_type, "context_length_exceeded" | "request_too_large") {
        return true;
    }
    let haystack = message.to_ascii_lowercase();
    haystack.contains("context")
        && [
            "length",
            "window",
            "exceed",
            "too long",
            "too large",
            "maximum",
        ]
        .iter()
        .any(|pattern| haystack.contains(pattern))
}

/// 读 retry-after（数字秒或 HTTP 日期；非正数 / 解析不了返回 None）。
fn retry_after_ms(headers: Option<&reqwest::header::HeaderMap>) -> Option<u64> {
    let value = headers?.get(reqwest::header::RETRY_AFTER)?;
    let text = value.to_str().ok()?;
    parse_retry_after(text.trim(), 0)
}

/// 解析 retry-after 的值（DSH llm-deepseek/src/transport.ts:36-37：数字秒或 HTTP 日期，
/// 换成毫秒；非正数返回 None 让重试层回退本地退避）。骨架只认数字秒。
fn parse_retry_after(text: &str, _now_ms: u64) -> Option<u64> {
    let seconds: f64 = text.parse().ok()?;
    if seconds.is_finite() && seconds > 0.0 {
        Some((seconds * 1000.0).round() as u64)
    } else {
        None
    }
}

/// 把 reqwest 的传输错误归类。
pub(super) fn transport_error(source: reqwest::Error) -> ModelError {
    let code = if source.is_timeout() {
        ErrorCode::Timeout
    } else {
        ErrorCode::Transport
    };
    let status = source.status().map(|status| status.as_u16());
    let mut error = ModelError::new(code, format!("DeepSeek Messages 传输失败: {source}"));
    error.status = status;
    error
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    /// 传输超时归 TIMEOUT（K10 点名项）。
    #[tokio::test]
    async fn timeout_maps_to_timeout_code() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        // 收下请求但永不写响应；客户端 100ms 超时。
        let _server = std::thread::spawn(move || {
            use std::io::Read;
            if let Ok((mut stream, _)) = listener.accept() {
                let mut buffer = [0u8; 1024];
                let _ = stream.read(&mut buffer);
                std::thread::sleep(Duration::from_secs(3));
            }
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let error = client
            .get(format!("http://127.0.0.1:{port}/v1/messages"))
            .send()
            .await
            .expect_err("永不响应的服务应超时");
        let mapped = transport_error(error);
        assert_eq!(mapped.code, ErrorCode::Timeout);
    }

    /// 连不上归 TRANSPORT（K10 点名项）。
    #[tokio::test]
    async fn connection_error_maps_to_transport_code() {
        // 绑一个端口再关掉，保证这个端口上没有服务。
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let client = reqwest::Client::new();
        let error = client
            .get(format!("http://127.0.0.1:{port}/v1/messages"))
            .send()
            .await
            .expect_err("没有服务时应连接失败");
        let mapped = transport_error(error);
        assert_eq!(mapped.code, ErrorCode::Transport);
    }

    /// retry-after 数字秒照旧（DSH transport.ts:36-37 的数值分支，允许小数）。
    #[test]
    fn retry_after_accepts_numeric_seconds() {
        assert_eq!(parse_retry_after("2", 0), Some(2_000));
        assert_eq!(parse_retry_after("0.5", 0), Some(500));
        assert_eq!(parse_retry_after("60", 0), Some(60_000));
        assert_eq!(parse_retry_after("0", 0), None, "非正数交回本地退避");
        assert_eq!(parse_retry_after("-1", 0), None);
        assert_eq!(parse_retry_after("bogus", 0), None);
    }

    /// T006.14（DSH transport.ts:36-37 用 Date.parse）：retry-after 也接受 HTTP-date
    /// 三种形式（RFC 9110 §5.6.7），换算成相对 now 的毫秒；过去时间返回 None。
    #[test]
    fn retry_after_accepts_http_date_forms() {
        // 1994-11-06T08:49:37Z 是 RFC 9110 的示例日期。
        let epoch_ms = 784_111_777_000u64;
        let now_ms = epoch_ms - 60_000;
        for value in [
            "Sun, 06 Nov 1994 08:49:37 GMT",
            "Sunday, 06-Nov-94 08:49:37 GMT",
            "Sun Nov  6 08:49:37 1994",
        ] {
            assert_eq!(
                parse_retry_after(value, now_ms),
                Some(60_000),
                "HTTP-date {value} 应换算成 60s"
            );
        }
        assert_eq!(
            parse_retry_after("Sun, 06 Nov 1994 08:49:37 GMT", epoch_ms + 1_000),
            None,
            "过去的日期交回本地退避（DSH 只认 >0 的 provider 延迟）"
        );
    }
}
