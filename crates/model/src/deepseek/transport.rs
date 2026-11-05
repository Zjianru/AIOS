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
    parse_retry_after(text.trim(), now_ms())
}

/// 当前 unix 毫秒（HTTP 日期换成相对延迟用）。
fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| {
            elapsed.as_millis().try_into().unwrap_or(u64::MAX)
        })
}

/// 解析 retry-after 的值（DSH llm-deepseek/src/transport.ts:36-37：数字秒或 HTTP 日期，
/// 换成毫秒；非正数返回 None 让重试层回退本地退避）。
fn parse_retry_after(text: &str, now_ms: u64) -> Option<u64> {
    if let Ok(seconds) = text.parse::<f64>() {
        return (seconds.is_finite() && seconds > 0.0).then(|| (seconds * 1000.0).round() as u64);
    }
    let epoch_ms = parse_http_date_ms(text)?;
    let delay = epoch_ms.checked_sub(now_ms)?;
    (delay > 0).then_some(delay)
}

/// 解析 HTTP-date（RFC 9110 §5.6.7 的三种形式，和 DSH 用 Date.parse 接受的范围对齐）：
/// IMF-fixdate（Sun, 06 Nov 1994 08:49:37 GMT）、RFC 850（Sunday, 06-Nov-94 08:49:37 GMT）、
/// asctime（Sun Nov  6 08:49:37 1994）。返回 unix 毫秒。
fn parse_http_date_ms(text: &str) -> Option<u64> {
    let (weekday, rest) = match text.split_once(", ") {
        Some((weekday, rest)) if !weekday.is_empty() => (weekday, rest),
        _ => ("", text),
    };
    let _ = weekday;
    if let Some(rest) = rest.strip_suffix(" GMT") {
        let fields: Vec<&str> = rest.split_whitespace().collect();
        return match fields.as_slice() {
            // IMF-fixdate：06 Nov 1994 08:49:37
            [day, month, year, time] if !year.contains('-') && !year.contains(':') => {
                let (hour, minute, second) = parse_hms(time)?;
                date_time_ms(
                    parse_u32(day)?,
                    month_number(month)?,
                    year.parse().ok()?,
                    hour,
                    minute,
                    second,
                )
            }
            // RFC 850：06-Nov-94 08:49:37
            [date, time] => {
                let mut parts = date.split('-');
                let day = parts.next()?;
                let month = parts.next()?;
                let year = two_digit_year(parts.next()?)?;
                if parts.next().is_some() {
                    return None;
                }
                let (hour, minute, second) = parse_hms(time)?;
                date_time_ms(
                    parse_u32(day)?,
                    month_number(month)?,
                    year,
                    hour,
                    minute,
                    second,
                )
            }
            _ => None,
        };
    }
    // asctime：Sun Nov  6 08:49:37 1994
    let fields: Vec<&str> = text.split_whitespace().collect();
    let [_, month, day, time, year] = fields.as_slice() else {
        return None;
    };
    let (hour, minute, second) = parse_hms(time)?;
    date_time_ms(
        parse_u32(day)?,
        month_number(month)?,
        year.parse().ok()?,
        hour,
        minute,
        second,
    )
}

fn parse_u32(text: &str) -> Option<u32> {
    text.parse().ok()
}

/// RFC 9110 §5.6.7：两位年份 70–99 是 19xx，00–69 是 20xx（同 JS Date.parse 的取舍）。
fn two_digit_year(text: &str) -> Option<i32> {
    let year: i32 = text.parse().ok()?;
    if !(0..=99).contains(&year) {
        return None;
    }
    Some(if year >= 70 { 1900 + year } else { 2000 + year })
}

fn month_number(month: &str) -> Option<u32> {
    Some(match month {
        "Jan" => 1,
        "Feb" => 2,
        "Mar" => 3,
        "Apr" => 4,
        "May" => 5,
        "Jun" => 6,
        "Jul" => 7,
        "Aug" => 8,
        "Sep" => 9,
        "Oct" => 10,
        "Nov" => 11,
        "Dec" => 12,
        _ => return None,
    })
}

fn parse_hms(text: &str) -> Option<(u32, u32, u32)> {
    let mut parts = text.split(':');
    let hour: u32 = parts.next()?.parse().ok()?;
    let minute: u32 = parts.next()?.parse().ok()?;
    let second: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    Some((hour, minute, second))
}

/// (day, month, year, h, m, s) → unix 毫秒；日期非法返回 None。
fn date_time_ms(
    day: u32,
    month: u32,
    year: i32,
    hour: u32,
    minute: u32,
    second: u32,
) -> Option<u64> {
    let days = days_from_civil(year, month, day)?;
    let seconds = days
        .checked_mul(86_400)?
        .checked_add(i64::from(hour) * 3_600 + i64::from(minute) * 60 + i64::from(second))?;
    u64::try_from(seconds).ok().map(|seconds| seconds * 1_000)
}

/// Howard Hinnant 的 days_from_civil：公历日期 → 1970-01-01 起的天数（含闰年校验）。
fn days_from_civil(year: i32, month: u32, day: u32) -> Option<i64> {
    let leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
    let max_day = match month {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return None,
    };
    if day == 0 || day > max_day {
        return None;
    }
    let month = i64::from(month);
    let year = i64::from(year) - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    Some(era * 146_097 + day_of_era - 719_468)
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
                // T009.H6：测试服务器收请求后保持不响应，等客户端 100ms 超时；3s 保持线程存活
                std::thread::sleep(Duration::from_secs(3));
            }
        });
        let client = reqwest::Client::builder()
            .timeout(Duration::from_millis(100))
            .no_proxy()
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
        let client = reqwest::Client::builder().no_proxy().build().unwrap();
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
