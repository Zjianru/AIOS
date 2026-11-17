//! 触发规则的中文事实：确认卡、日历页、aios_query 共用这一份，不再各拼 RRULE 原文。
//!
//! 只翻服务支持的几类；认不出就回规则原文兜底（不硬猜）。

use std::collections::BTreeMap;

use chrono::DateTime;

/// 把一条 RRULE + 时区 + 起点说成中文；一次/每天/工作日/每周几/每月几号/每 N 小时。
pub(crate) fn describe(rule: &str, timezone: &str, start_at: i64) -> String {
    let zone: chrono_tz::Tz = match timezone.parse() {
        Ok(zone) => zone,
        Err(_) => return rule.to_owned(),
    };
    let Some(start) = DateTime::from_timestamp_millis(start_at) else {
        return rule.to_owned();
    };
    let start = start.with_timezone(&zone);
    let suffix = format!("（{timezone}）");
    let parts: BTreeMap<&str, &str> = rule
        .split(';')
        .filter_map(|part| part.split_once('='))
        .collect();
    let count: Option<u32> = parts.get("COUNT").and_then(|value| value.parse().ok());
    if count == Some(1) {
        return format!("一次：{}（{timezone}）", start.format("%Y-%m-%d %H:%M"));
    }
    match parts.get("FREQ").copied() {
        Some("DAILY") => format!("每天 {}{suffix}", start.format("%H:%M")),
        Some("WEEKLY") => weekly(parts.get("BYDAY").copied(), start, &suffix),
        Some("MONTHLY") => monthly(parts.get("BYMONTHDAY").copied(), start, &suffix),
        Some("HOURLY") => {
            let interval: u32 = parts
                .get("INTERVAL")
                .and_then(|value| value.parse().ok())
                .unwrap_or(1);
            format!("每 {interval} 小时{suffix}")
        }
        _ => rule.to_owned(),
    }
}

fn weekly(byday: Option<&str>, start: DateTime<chrono_tz::Tz>, suffix: &str) -> String {
    let days: Vec<&str> = byday
        .map(|value| value.split(',').collect())
        .unwrap_or_default();
    if days == ["MO", "TU", "WE", "TH", "FR"] {
        return format!("工作日 {}{suffix}", start.format("%H:%M"));
    }
    let names: Vec<&str> = days.iter().filter_map(|day| weekday(day)).collect();
    if names.is_empty() {
        // 没写 BYDAY：按起点那天的星期几，仍是每周一次。
        return format!(
            "每周{} {}{suffix}",
            weekday_of(&start),
            start.format("%H:%M")
        );
    }
    if names.len() == 1 {
        return format!("每周{} {}{suffix}", names[0], start.format("%H:%M"));
    }
    format!("每周{} {}{suffix}", names.join("、"), start.format("%H:%M"))
}

fn monthly(by_month_day: Option<&str>, start: DateTime<chrono_tz::Tz>, suffix: &str) -> String {
    let day = by_month_day
        .and_then(|value| value.split(',').next())
        .and_then(|value| value.parse::<u32>().ok());
    match day {
        Some(day) => format!("每月 {day} 号 {}{suffix}", start.format("%H:%M")),
        None => format!("每月{}{suffix}", start.format("%H:%M")),
    }
}

fn weekday(code: &str) -> Option<&'static str> {
    match code {
        "MO" => Some("一"),
        "TU" => Some("二"),
        "WE" => Some("三"),
        "TH" => Some("四"),
        "FR" => Some("五"),
        "SA" => Some("六"),
        "SU" => Some("日"),
        _ => None,
    }
}

fn weekday_of(start: &DateTime<chrono_tz::Tz>) -> &'static str {
    use chrono::Datelike;
    ["一", "二", "三", "四", "五", "六", "日"]
        [(start.weekday().num_days_from_monday() as usize).min(6)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(zone: &str, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        let tz: chrono_tz::Tz = zone.parse().unwrap();
        tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn supported_rules_have_chinese_text_with_timezone() {
        let shanghai = at("Asia/Shanghai", 2026, 11, 18, 9, 0);
        assert_eq!(
            describe("FREQ=DAILY;COUNT=1", "Asia/Shanghai", shanghai),
            "一次：2026-11-18 09:00（Asia/Shanghai）"
        );
        assert_eq!(
            describe("FREQ=DAILY", "Asia/Shanghai", shanghai),
            "每天 09:00（Asia/Shanghai）"
        );
        assert_eq!(
            describe(
                "FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR",
                "Asia/Shanghai",
                shanghai
            ),
            "工作日 09:00（Asia/Shanghai）"
        );
        assert_eq!(
            describe("FREQ=WEEKLY;BYDAY=MO,WE", "Asia/Shanghai", shanghai),
            "每周一、三 09:00（Asia/Shanghai）"
        );
        assert_eq!(
            describe("FREQ=WEEKLY;BYDAY=MO", "Asia/Shanghai", shanghai),
            "每周一 09:00（Asia/Shanghai）"
        );
        assert_eq!(
            describe("FREQ=MONTHLY;BYMONTHDAY=31", "Asia/Shanghai", shanghai),
            "每月 31 号 09:00（Asia/Shanghai）"
        );
        assert_eq!(
            describe("FREQ=HOURLY;INTERVAL=6", "Asia/Shanghai", shanghai),
            "每 6 小时（Asia/Shanghai）"
        );
    }

    #[test]
    fn unsupported_rule_falls_back_to_original_text() {
        assert_eq!(describe("FREQ=YEARLY", "Asia/Shanghai", 0), "FREQ=YEARLY");
    }
}
