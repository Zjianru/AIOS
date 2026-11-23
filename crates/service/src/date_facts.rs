//! T077.6：Kind 每轮末尾的「现在」程序消息，和约定到点消息的本地时间/晚多久。
//!
//! 时区取本机：TZ、/etc/localtime 指向的 zoneinfo 名，最后退回当前偏移。
//! 固定块不在这里改；这里只产出纯文本事实。

use chrono::{Local, TimeZone};

/// 本机时区名：优先 TZ，其次 /etc/localtime 的 zoneinfo 路径，最后用当前偏移。
#[cfg(test)]
pub(crate) fn timezone_name() -> String {
    if let Ok(tz) = std::env::var("TZ")
        && !tz.trim().is_empty()
    {
        return tz;
    }
    if let Ok(link) = std::fs::read_link("/etc/localtime")
        && let Some(name) = link
            .to_str()
            .and_then(|path| path.split("zoneinfo/").nth(1))
            .filter(|name| !name.is_empty())
    {
        return name.to_owned();
    }
    Local::now().offset().to_string()
}

pub(crate) fn local_clock(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Local::now)
        .format("%Y-%m-%d %H:%M")
        .to_string()
}

/// 每轮末尾的纯文本：现在：2026-11-23 13:30（Asia/Shanghai）。
#[cfg(test)]
pub(crate) fn now_line(now_ms: i64) -> String {
    format!("现在：{}（{}）", local_clock(now_ms), timezone_name())
}

/// 人话时长：45 秒 / 2 分钟 / 1 小时 5 分钟 / 2 天。
pub(crate) fn human_duration(ms: i64) -> String {
    let seconds = (ms.max(0) / 1000) as u64;
    if seconds < 60 {
        return format!("{seconds} 秒");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes} 分钟");
    }
    let hours = minutes / 60;
    if hours < 24 {
        let rest = minutes % 60;
        return if rest == 0 {
            format!("{hours} 小时")
        } else {
            format!("{hours} 小时 {rest} 分钟")
        };
    }
    let days = hours / 24;
    let rest = hours % 24;
    if rest == 0 {
        format!("{days} 天")
    } else {
        format!("{days} 天 {rest} 小时")
    }
}

/// 约定时间的本地人和话：准时 / 比约定时间晚了 N。
pub(crate) fn late_text(nominal_at: i64, now_ms: i64) -> String {
    let late = now_ms.saturating_sub(nominal_at);
    if late < 1_000 {
        "准时".to_owned()
    } else {
        format!("比约定时间晚了 {}", human_duration(late))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duration_reads_like_a_person() {
        assert_eq!(human_duration(0), "0 秒");
        assert_eq!(human_duration(45_000), "45 秒");
        assert_eq!(human_duration(125_000), "2 分钟");
        assert_eq!(human_duration(3_900_000), "1 小时 5 分钟");
        assert_eq!(human_duration(90_000_000), "1 天 1 小时");
    }

    #[test]
    fn late_text_has_the_required_phrase() {
        assert_eq!(late_text(1_000, 1_500), "准时");
        assert_eq!(late_text(1_000, 126_000), "比约定时间晚了 2 分钟");
    }

    #[test]
    fn now_line_has_local_clock_and_timezone() {
        let line = now_line(1_700_000_000_000);
        assert!(line.starts_with("现在："));
        assert!(line.contains('（'));
        assert!(line.ends_with('）'));
        assert!(!timezone_name().is_empty());
    }
}
