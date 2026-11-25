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
    if matches!(status, Some(429 | 402)) {
        error.resume_at_ms = resume_at_ms(headers, error.retry_after_ms);
    }
    error
}

/// T012.6：400 是不是厂商在拒绝「关掉思考」（关不掉思考 / 必须思考）。
///
/// 照 Magpie gateway/anthropic.go:425 的 alwaysThinks 与 thinking_needed.go 的
/// thinkingNeededRefusal（只认 Messages 协议会遇到的文案；中英文都认）。
pub(super) fn refuses_thinking_off(raw: &Value) -> bool {
    let message = raw
        .pointer("/error/message")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let lower = message.to_ascii_lowercase();
    // MiniMax 的 2013 出现在错误原文里（Magpie 的 alwaysThinks 行）。
    if lower.contains("2013") {
        return true;
    }
    const FRAGMENTS: &[&str] = &[
        "always engages in thinking",
        "thinking cannot be disabled",
        "thinking can't be disabled",
        "thinking cannot be turned off",
        "requires adaptive thinking",
        "thinking.type must be enabled",
        "thinking.type `disabled` is not supported",
        "enable_thinking parameter is restricted to true",
    ];
    if FRAGMENTS.iter().any(|fragment| lower.contains(fragment)) {
        return true;
    }
    // thinkingNeededRefusal：thinking.type must/should/has to be "enabled"。
    let thinking_needed = lower.contains("thinking")
        && (lower.contains("must be")
            || lower.contains("should be")
            || lower.contains("has to be"))
        && lower.contains("enabled");
    if thinking_needed {
        return true;
    }
    // 中文：始终思考 / 思考不可关闭、无法关闭、不能关闭。
    (message.contains("始终") && message.contains("思考"))
        || (message.contains("思考")
            && (message.contains("不可关闭")
                || message.contains("无法关闭")
                || message.contains("不能关闭")))
}

/// T012.6：401 是不是厂商在拒绝「x-api-key 和 Authorization 一起带」。
///
/// 照 Magpie catalog.BothAuthRefused：401 且错误体同时提到两个头、并带拒绝同时存在的措辞
/// （中英文）。
pub(super) fn both_auth_refused(status: u16, raw: &Value) -> bool {
    if status != 401 {
        return false;
    }
    let text = raw.to_string().to_ascii_lowercase();
    if !text.contains("x-api-key") || !text.contains("authorization") {
        return false;
    }
    const PHRASES: &[&str] = &[
        "同时",
        "并存",
        "共存",
        "只能使用一个",
        "只能选择一个",
        "只能有一个",
        "cannot both",
        "can't both",
        "can not both",
        "cannot be both",
        "not both",
        "together",
        "at the same time",
        "simultaneous",
        "mutually exclusive",
        "only one of",
        "only one auth",
        "only one header",
        "both present",
        "both be present",
        "both provided",
        "both be provided",
    ];
    PHRASES.iter().any(|phrase| text.contains(phrase))
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

/// 429/402 的绝对恢复时刻：Retry-After 优先，其次 Anthropic ratelimit reset 头。
///
/// reset 头只在「撞满」的桶里取最晚值；没有 remaining=0 时退回所有 reset 的最晚值。
/// 只认响应头，不读错误正文，不按供应商窗口推算。
fn resume_at_ms(
    headers: Option<&reqwest::header::HeaderMap>,
    retry_after_ms: Option<u64>,
) -> Option<i64> {
    let now = now_ms();
    if let Some(delay) = retry_after_ms {
        return i64::try_from(now.saturating_add(delay)).ok();
    }
    let headers = headers?;
    let now = i64::try_from(now).ok()?;
    let mut exhausted: Vec<i64> = Vec::new();
    let mut all: Vec<i64> = Vec::new();
    for (reset_name, remaining_name) in [
        (
            "anthropic-ratelimit-requests-reset",
            "anthropic-ratelimit-requests-remaining",
        ),
        (
            "anthropic-ratelimit-tokens-reset",
            "anthropic-ratelimit-tokens-remaining",
        ),
        (
            "anthropic-ratelimit-input-tokens-reset",
            "anthropic-ratelimit-input-tokens-remaining",
        ),
        (
            "anthropic-ratelimit-output-tokens-reset",
            "anthropic-ratelimit-output-tokens-remaining",
        ),
    ] {
        let Some(reset) = headers
            .get(reset_name)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_rfc3339_ms)
            .and_then(|at| i64::try_from(at).ok())
        else {
            continue;
        };
        if reset <= now {
            continue;
        }
        all.push(reset);
        if headers
            .get(remaining_name)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.trim().parse::<u64>().ok())
            .is_some_and(|remaining| remaining == 0)
        {
            exhausted.push(reset);
        }
    }
    exhausted
        .into_iter()
        .max()
        .or_else(|| all.into_iter().max())
}

/// RFC 3339（Z 或 +-HH:MM 偏移，可带小数秒）→ unix 毫秒；其它形态返回 None。
fn parse_rfc3339_ms(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T').or_else(|| text.split_once(' '))?;
    let mut parts = date.split('-');
    let year: i32 = parts.next()?.parse().ok()?;
    let month: u32 = parts.next()?.parse().ok()?;
    let day: u32 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    let (clock, offset_seconds) = if let Some(clock) = rest.strip_suffix('Z') {
        (clock, 0i64)
    } else if let Some((clock, offset)) = rest.rsplit_once('+') {
        (clock, parse_offset(offset)?)
    } else if let Some((clock, offset)) = rest.rsplit_once('-') {
        (clock, -parse_offset(offset)?)
    } else {
        return None;
    };
    let (clock, fraction_ms) = match clock.split_once('.') {
        Some((clock, fraction)) => {
            if fraction.is_empty() || !fraction.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            let mut millis = fraction
                .as_bytes()
                .iter()
                .take(3)
                .fold(0u32, |acc, byte| acc * 10 + u32::from(byte - b'0'));
            for _ in fraction.len().min(3)..3 {
                millis *= 10;
            }
            (clock, millis)
        }
        None => (clock, 0),
    };
    let (hour, minute, second) = parse_hms(clock)?;
    let base = i64::try_from(date_time_ms(day, month, year, hour, minute, second)?).ok()?;
    let adjusted = base
        .checked_add(i64::from(fraction_ms))?
        .checked_sub(offset_seconds.checked_mul(1_000)?)?;
    u64::try_from(adjusted).ok()
}

/// RFC 3339 的 +-HH:MM / +-HHMM 偏移 → 秒。
fn parse_offset(text: &str) -> Option<i64> {
    let (hours, minutes) = match text.split_once(':') {
        Some((hours, minutes)) => (hours, minutes),
        None if text.len() == 4 => (&text[..2], &text[2..]),
        None => return None,
    };
    let hours: i64 = hours.parse().ok()?;
    let minutes: i64 = minutes.parse().ok()?;
    if !(0..=23).contains(&hours) || !(0..=59).contains(&minutes) {
        return None;
    }
    Some(hours * 3_600 + minutes * 60)
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
                #[expect(
                    clippy::let_underscore_must_use,
                    reason = "测试服务器不读请求内容，读失败不影响超时用例"
                )]
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

    fn header_map(pairs: &[(&str, &str)]) -> reqwest::header::HeaderMap {
        let mut map = reqwest::header::HeaderMap::new();
        for (name, value) in pairs {
            map.insert(
                reqwest::header::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                reqwest::header::HeaderValue::from_str(value).unwrap(),
            );
        }
        map
    }

    fn error_with(headers: &reqwest::header::HeaderMap, status: u16) -> ModelError {
        provider_error(
            &serde_json::json!({
                "error": { "type": "rate_limit_error", "message": "限流" }
            }),
            Some(status),
            Some(headers),
        )
    }

    #[test]
    fn rfc3339_parses_z_offset_and_fraction() {
        let utc = date_time_ms(25, 11, 2026, 12, 0, 0).unwrap();
        assert_eq!(parse_rfc3339_ms("2026-11-25T12:00:00Z"), Some(utc));
        assert_eq!(parse_rfc3339_ms("2026-11-25T20:00:00+08:00"), Some(utc));
        assert_eq!(
            parse_rfc3339_ms("2026-11-25T12:00:00.250Z"),
            Some(utc + 250)
        );
        assert_eq!(parse_rfc3339_ms("2026-11-25T12:00:00"), None);
        assert_eq!(parse_rfc3339_ms("bogus"), None);
    }

    #[test]
    fn retry_after_is_the_absolute_resume_when_present() {
        let headers = header_map(&[("retry-after", "20")]);
        let before = i64::try_from(now_ms()).unwrap();
        let resume = error_with(&headers, 429)
            .resume_at_ms
            .expect("短/长都存绝对时刻");
        assert!(
            (resume - before - 20_000).abs() < 2_000,
            "resume 应是 now+20s：{resume} vs {before}"
        );
    }

    #[test]
    fn retry_after_wins_over_reset_headers() {
        let headers = header_map(&[
            ("retry-after", "20"),
            ("anthropic-ratelimit-requests-reset", "2099-01-01T00:00:00Z"),
            ("anthropic-ratelimit-requests-remaining", "0"),
        ]);
        let before = i64::try_from(now_ms()).unwrap();
        let resume = error_with(&headers, 429)
            .resume_at_ms
            .expect("Retry-After 优先");
        assert!(
            (resume - before - 20_000).abs() < 2_000,
            "{resume} vs {before}"
        );
    }

    #[test]
    fn reset_headers_choose_the_latest_exhausted_bucket() {
        let headers = header_map(&[
            ("anthropic-ratelimit-requests-reset", "2099-01-01T00:00:00Z"),
            ("anthropic-ratelimit-requests-remaining", "0"),
            ("anthropic-ratelimit-tokens-reset", "2099-01-01T01:00:00Z"),
            ("anthropic-ratelimit-tokens-remaining", "0"),
            (
                "anthropic-ratelimit-input-tokens-reset",
                "2099-01-01T00:30:00Z",
            ),
            ("anthropic-ratelimit-input-tokens-remaining", "3"),
        ]);
        let resume = error_with(&headers, 429)
            .resume_at_ms
            .expect("reset 里的恢复时间");
        assert_eq!(
            resume,
            i64::try_from(date_time_ms(1, 1, 2099, 1, 0, 0).unwrap()).unwrap(),
            "撞满的桶取最晚；没撞满的不优先"
        );
    }

    #[test]
    fn reset_headers_fall_back_to_latest_when_remaining_is_missing() {
        let headers = header_map(&[
            ("anthropic-ratelimit-requests-reset", "2099-01-01T00:00:00Z"),
            ("anthropic-ratelimit-tokens-reset", "2099-01-01T02:00:00Z"),
        ]);
        let resume = error_with(&headers, 402).resume_at_ms.expect("402 也认");
        assert_eq!(
            resume,
            i64::try_from(date_time_ms(1, 1, 2099, 2, 0, 0).unwrap()).unwrap()
        );
    }

    #[test]
    fn non_429_402_and_invalid_headers_have_no_resume() {
        let headers = header_map(&[("retry-after", "20")]);
        assert!(error_with(&headers, 500).resume_at_ms.is_none());
        let headers = header_map(&[("retry-after", "bogus")]);
        assert!(error_with(&headers, 429).resume_at_ms.is_none());
        let headers = header_map(&[("anthropic-ratelimit-requests-reset", "2000-01-01T00:00:00Z")]);
        assert!(
            error_with(&headers, 429).resume_at_ms.is_none(),
            "过期 reset 不安排"
        );
        assert!(error_with(&headers, 429).status.is_some());
    }
}
