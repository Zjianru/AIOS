//! T077.6：约定到点消息的本地时间和「晚了多久」。
//!
//! 「现在」程序消息的格式在 agent/src/fixed_block.rs 一处；这里只保留约定事实。

use chrono::{Local, TimeZone};

pub(crate) fn local_clock(ms: i64) -> String {
    Local
        .timestamp_millis_opt(ms)
        .single()
        .unwrap_or_else(Local::now)
        .format("%Y-%m-%d %H:%M")
        .to_string()
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
}
