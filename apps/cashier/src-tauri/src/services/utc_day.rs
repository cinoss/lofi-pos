//! UTC calendar-day helpers used by the key rotation pipeline.
//!
//! These are deliberately distinct from `business_day_of` (which depends on the
//! venue's timezone + cutoff hour). The crypto lifecycle is UTC-driven so it is
//! independent of any operator-configurable settings.

use chrono::{Duration, NaiveDate, TimeZone, Utc};

/// `YYYY-MM-DD` of `ts_ms` interpreted as UTC.
pub fn utc_day_of(ts_ms: i64) -> String {
    Utc.timestamp_millis_opt(ts_ms)
        .unwrap()
        .format("%Y-%m-%d")
        .to_string()
}

/// Wall-clock millis of the next UTC midnight strictly after `now_ms`.
/// If `now_ms` is exactly at UTC midnight, returns the following midnight.
pub fn next_utc_midnight_ms(now_ms: i64) -> i64 {
    let dt = Utc.timestamp_millis_opt(now_ms).unwrap();
    let next = (dt.date_naive() + Duration::days(1))
        .and_hms_opt(0, 0, 0)
        .unwrap();
    Utc.from_utc_datetime(&next).timestamp_millis()
}

/// `day` minus `n` UTC days, formatted as `YYYY-MM-DD`.
pub fn days_ago(day: &str, n: i64) -> String {
    let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
    (d - Duration::days(n)).format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(y, m, d, h, mi, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn epoch_is_1970_01_01() {
        assert_eq!(utc_day_of(0), "1970-01-01");
    }

    #[test]
    fn just_before_midnight_is_today() {
        assert_eq!(utc_day_of(ts(2026, 4, 28, 23, 59)), "2026-04-28");
    }

    #[test]
    fn at_midnight_starts_new_day() {
        assert_eq!(utc_day_of(ts(2026, 4, 29, 0, 0)), "2026-04-29");
    }

    #[test]
    fn local_offset_does_not_affect_utc_day() {
        // 2026-04-28 23:00 UTC == 2026-04-29 06:00 +07; UTC day is the 28th.
        assert_eq!(utc_day_of(ts(2026, 4, 28, 23, 0)), "2026-04-28");
    }

    #[test]
    fn next_utc_midnight_after_now() {
        let now = ts(2026, 4, 28, 14, 30);
        let next = next_utc_midnight_ms(now);
        assert_eq!(next, ts(2026, 4, 29, 0, 0));
    }

    #[test]
    fn at_utc_midnight_returns_next_one() {
        let now = ts(2026, 4, 29, 0, 0);
        let next = next_utc_midnight_ms(now);
        assert_eq!(next, ts(2026, 4, 30, 0, 0));
    }

    #[test]
    fn days_ago_subtracts_correctly() {
        assert_eq!(days_ago("2026-04-28", 3), "2026-04-25");
        assert_eq!(days_ago("2026-04-28", 0), "2026-04-28");
    }
}
