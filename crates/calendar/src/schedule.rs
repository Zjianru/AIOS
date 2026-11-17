//! 规则计算：RRULE + IANA 时区；只吃传入的时钟，不读系统时钟。

use chrono::TimeZone;

use crate::error::CalendarError;
use crate::types::Schedule;

impl Schedule {
    /// 规则本身能不能读出来、时区认不认识；建/改约定时先校验。
    pub fn validate(&self) -> Result<(), CalendarError> {
        self.rule_set().map(|_| ())
    }

    /// 严格晚于 after_ms 的下一次触发（毫秒）；没有下一次返回 None。
    pub fn next_after(&self, after_ms: i64) -> Result<Option<i64>, CalendarError> {
        let tz: rrule::Tz = self.timezone()?.into();
        let after = tz
            .timestamp_millis_opt(after_ms)
            .single()
            .ok_or_else(|| CalendarError::InvalidSchedule("当前时间戳不在时区范围内".to_owned()))?;
        let result = self.rule_set()?.after(after).all(2);
        Ok(result
            .dates
            .iter()
            .map(chrono::DateTime::timestamp_millis)
            .find(|at| *at > after_ms))
    }

    /// [from_ms, to_ms] 之间的触发点，按时间升序；周视图的计划点用。
    pub fn occurrences_between(&self, from_ms: i64, to_ms: i64) -> Result<Vec<i64>, CalendarError> {
        let tz: rrule::Tz = self.timezone()?.into();
        let from = tz
            .timestamp_millis_opt(from_ms)
            .single()
            .ok_or_else(|| CalendarError::InvalidSchedule("起点不在时区范围内".to_owned()))?;
        let to = tz
            .timestamp_millis_opt(to_ms)
            .single()
            .ok_or_else(|| CalendarError::InvalidSchedule("终点不在时区范围内".to_owned()))?;
        let result = self.rule_set()?.after(from).before(to).all(1000);
        Ok(result
            .dates
            .iter()
            .map(chrono::DateTime::timestamp_millis)
            .filter(|at| *at >= from_ms && *at <= to_ms)
            .collect())
    }

    fn timezone(&self) -> Result<chrono_tz::Tz, CalendarError> {
        self.timezone
            .parse()
            .map_err(|_| CalendarError::InvalidTimezone(self.timezone.clone()))
    }

    fn rule_set(&self) -> Result<rrule::RRuleSet, CalendarError> {
        let timezone: rrule::Tz = self.timezone()?.into();
        let start_at = timezone
            .timestamp_millis_opt(self.start_at)
            .single()
            .ok_or_else(|| CalendarError::InvalidSchedule("起点不在时区范围内".to_owned()))?;
        let rule: rrule::RRule<rrule::Unvalidated> = self
            .rule
            .parse()
            .map_err(|_| CalendarError::InvalidRule(self.rule.clone()))?;
        let rule = rule
            .validate(start_at)
            .map_err(|error| CalendarError::InvalidSchedule(error.to_string()))?;
        Ok(rrule::RRuleSet::new(start_at).rrule(rule))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Tz;

    fn at(zone: &str, year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
        let tz: Tz = zone.parse().expect("timezone");
        tz.with_ymd_and_hms(year, month, day, hour, minute, 0)
            .single()
            .expect("valid local time")
            .timestamp_millis()
    }

    fn schedule(rule: &str, zone: &str, hour: u32, minute: u32) -> Schedule {
        Schedule {
            rule: rule.to_owned(),
            timezone: zone.to_owned(),
            start_at: at(zone, 2026, 1, 1, hour, minute),
        }
    }

    #[test]
    fn daily_at_nine_in_asia_shanghai() {
        let schedule = schedule("FREQ=DAILY", "Asia/Shanghai", 9, 0);
        assert_eq!(
            schedule
                .next_after(at("Asia/Shanghai", 2026, 11, 18, 8, 0))
                .unwrap(),
            Some(at("Asia/Shanghai", 2026, 11, 18, 9, 0))
        );
        assert_eq!(
            schedule
                .next_after(at("Asia/Shanghai", 2026, 11, 18, 9, 0))
                .unwrap(),
            Some(at("Asia/Shanghai", 2026, 11, 19, 9, 0))
        );
    }

    #[test]
    fn weekdays_skip_the_weekend() {
        let schedule = schedule("FREQ=WEEKLY;BYDAY=MO,TU,WE,TH,FR", "Asia/Shanghai", 9, 0);
        assert_eq!(
            schedule
                .next_after(at("Asia/Shanghai", 2026, 11, 20, 9, 0))
                .unwrap(),
            Some(at("Asia/Shanghai", 2026, 11, 23, 9, 0)),
            "周五 9 点之后下一次是周一"
        );
    }

    #[test]
    fn weekly_on_monday() {
        let schedule = schedule("FREQ=WEEKLY;BYDAY=MO", "Asia/Shanghai", 9, 0);
        assert_eq!(
            schedule
                .next_after(at("Asia/Shanghai", 2026, 11, 18, 0, 0))
                .unwrap(),
            Some(at("Asia/Shanghai", 2026, 11, 23, 9, 0))
        );
    }

    #[test]
    fn monthly_on_the_31st_skips_short_months() {
        let mut schedule = schedule("FREQ=MONTHLY;BYMONTHDAY=31", "Asia/Shanghai", 9, 0);
        schedule.start_at = at("Asia/Shanghai", 2026, 1, 31, 9, 0);
        assert_eq!(
            schedule
                .next_after(at("Asia/Shanghai", 2026, 1, 31, 9, 0))
                .unwrap(),
            Some(at("Asia/Shanghai", 2026, 3, 31, 9, 0)),
            "二月没有 31 号，下一次是三月 31"
        );
    }

    #[test]
    fn every_six_hours_uses_hourly_interval() {
        let mut schedule = schedule("FREQ=HOURLY;INTERVAL=6", "Asia/Shanghai", 0, 0);
        schedule.start_at = at("Asia/Shanghai", 2026, 11, 17, 0, 0);
        assert_eq!(
            schedule
                .next_after(at("Asia/Shanghai", 2026, 11, 17, 0, 30))
                .unwrap(),
            Some(at("Asia/Shanghai", 2026, 11, 17, 6, 0))
        );
    }

    #[test]
    fn los_angeles_keeps_local_nine_across_spring_forward() {
        let mut schedule = schedule("FREQ=DAILY", "America/Los_Angeles", 9, 0);
        schedule.start_at = at("America/Los_Angeles", 2026, 3, 1, 9, 0);
        assert_eq!(
            schedule
                .next_after(at("America/Los_Angeles", 2026, 3, 7, 10, 0))
                .unwrap(),
            Some(at("America/Los_Angeles", 2026, 3, 8, 9, 0)),
            "春令时切换后仍是本地 9 点"
        );
        assert_eq!(
            schedule
                .next_after(at("America/Los_Angeles", 2026, 3, 8, 9, 0))
                .unwrap(),
            Some(at("America/Los_Angeles", 2026, 3, 9, 9, 0))
        );
    }

    #[test]
    fn los_angeles_keeps_local_nine_across_fall_back() {
        let mut schedule = schedule("FREQ=DAILY", "America/Los_Angeles", 9, 0);
        schedule.start_at = at("America/Los_Angeles", 2026, 10, 25, 9, 0);
        assert_eq!(
            schedule
                .next_after(at("America/Los_Angeles", 2026, 10, 31, 10, 0))
                .unwrap(),
            Some(at("America/Los_Angeles", 2026, 11, 1, 9, 0))
        );
        assert_eq!(
            schedule
                .next_after(at("America/Los_Angeles", 2026, 11, 1, 9, 0))
                .unwrap(),
            Some(at("America/Los_Angeles", 2026, 11, 2, 9, 0))
        );
    }

    #[test]
    fn shanghai_next_trigger_does_not_shift_with_utc() {
        let schedule = schedule("FREQ=DAILY", "Asia/Shanghai", 9, 0);
        let first = schedule
            .next_after(at("UTC", 2026, 11, 17, 0, 0))
            .unwrap()
            .unwrap();
        let second = schedule.next_after(first).unwrap().unwrap();
        assert_eq!(second - first, 24 * 60 * 60 * 1000);
    }

    #[test]
    fn occurrences_between_returns_plan_points_in_order() {
        let schedule = schedule("FREQ=DAILY", "Asia/Shanghai", 9, 0);
        let points = schedule
            .occurrences_between(
                at("Asia/Shanghai", 2026, 11, 18, 0, 0),
                at("Asia/Shanghai", 2026, 11, 20, 23, 59),
            )
            .unwrap();
        assert_eq!(
            points,
            vec![
                at("Asia/Shanghai", 2026, 11, 18, 9, 0),
                at("Asia/Shanghai", 2026, 11, 19, 9, 0),
                at("Asia/Shanghai", 2026, 11, 20, 9, 0),
            ]
        );
    }

    #[test]
    fn invalid_timezone_and_rule_are_rejected() {
        let bad_zone = Schedule {
            rule: "FREQ=DAILY".to_owned(),
            timezone: "Mars/Olympus".to_owned(),
            start_at: at("Asia/Shanghai", 2026, 1, 1, 9, 0),
        };
        assert!(bad_zone.validate().is_err());
        let mut bad_rule = schedule("FREQ=DAILY", "Asia/Shanghai", 9, 0);
        bad_rule.rule = "NOT A RULE".to_owned();
        assert!(bad_rule.validate().is_err());
    }
}
