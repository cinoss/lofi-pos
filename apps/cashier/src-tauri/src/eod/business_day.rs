//! End-of-day arithmetic helpers in `(timestamp_ms, Cfg)` form. The existing
//! `crate::business_day` module exposes `business_day_of(DateTime<Utc>, …)` —
//! this module mirrors the same semantics on raw milliseconds for the EOD
//! pipeline (where everything flows through the `Clock` as `i64` ms).

use chrono::{DateTime, Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};

/// Business-day configuration. Mirrors the two settings that govern day
/// boundaries (`business_day_cutoff_hour`, `business_day_tz_offset_seconds`).
#[derive(Debug, Clone, Copy)]
pub struct Cfg {
    pub cutoff_hour: u32,
    pub tz_offset_seconds: i32,
}

/// The business day (`YYYY-MM-DD`) the timestamp belongs to.
///
/// Day `D` covers `[D cutoff_hour local, D+1 cutoff_hour local)`.
pub fn business_day_for(ts_ms: i64, cfg: Cfg) -> String {
    let local = local(ts_ms, cfg);
    let shifted = local - Duration::hours(cfg.cutoff_hour as i64);
    format!(
        "{:04}-{:02}-{:02}",
        shifted.year(),
        shifted.month(),
        shifted.day()
    )
}

/// UTC ms of the next cutoff strictly greater than `now_ms`.
pub fn next_cutoff_ms(now_ms: i64, cfg: Cfg) -> i64 {
    let local = local(now_ms, cfg);
    let cutoff_today = local
        .date_naive()
        .and_hms_opt(cfg.cutoff_hour, 0, 0)
        .expect("cutoff_hour in 0..24");
    let target_local = if local.naive_local() < cutoff_today {
        cutoff_today
    } else {
        cutoff_today + Duration::days(1)
    };
    let tz = FixedOffset::east_opt(cfg.tz_offset_seconds).expect("valid tz offset");
    tz.from_local_datetime(&target_local)
        .single()
        .expect("unambiguous local time at cutoff")
        .with_timezone(&Utc)
        .timestamp_millis()
}

/// All business days in `[from..to)` (exclusive end). Used by catch-up.
pub fn days_between(from: &str, to: &str) -> Vec<String> {
    let f = match NaiveDate::parse_from_str(from, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let t = match NaiveDate::parse_from_str(to, "%Y-%m-%d") {
        Ok(d) => d,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    let mut d = f;
    while d < t {
        out.push(d.format("%Y-%m-%d").to_string());
        d = match d.succ_opt() {
            Some(n) => n,
            None => break,
        };
    }
    out
}

fn local(ts_ms: i64, cfg: Cfg) -> DateTime<FixedOffset> {
    let tz = FixedOffset::east_opt(cfg.tz_offset_seconds).expect("valid tz offset");
    Utc.timestamp_millis_opt(ts_ms)
        .single()
        .expect("valid utc ms")
        .with_timezone(&tz)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    // tz offset = +7h (25200 sec). cutoff = 11.
    fn cfg() -> Cfg {
        Cfg {
            cutoff_hour: 11,
            tz_offset_seconds: 25200,
        }
    }

    fn ts_utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        Utc.with_ymd_and_hms(y, m, d, h, mi, s)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn before_cutoff_belongs_to_previous_day() {
        // 2026-04-28 03:59 UTC == 10:59 +07
        let ts = ts_utc(2026, 4, 28, 3, 59, 0);
        assert_eq!(business_day_for(ts, cfg()), "2026-04-27");
    }

    #[test]
    fn at_cutoff_starts_new_day() {
        let ts = ts_utc(2026, 4, 28, 4, 0, 0); // 11:00 +07
        assert_eq!(business_day_for(ts, cfg()), "2026-04-28");
    }

    #[test]
    fn next_cutoff_after_today() {
        // currently 2026-04-28 12:00 +07 (=05:00 UTC); next cutoff is 2026-04-29 11:00 +07 = 04:00 UTC
        let now = ts_utc(2026, 4, 28, 5, 0, 0);
        let next = next_cutoff_ms(now, cfg());
        assert_eq!(next, ts_utc(2026, 4, 29, 4, 0, 0));
    }

    #[test]
    fn next_cutoff_before_today() {
        // currently 2026-04-28 09:00 +07 (=02:00 UTC); next cutoff is today 2026-04-28 11:00 +07 = 04:00 UTC
        let now = ts_utc(2026, 4, 28, 2, 0, 0);
        let next = next_cutoff_ms(now, cfg());
        assert_eq!(next, ts_utc(2026, 4, 28, 4, 0, 0));
    }

    #[test]
    fn days_between_inclusive_exclusive() {
        let v = days_between("2026-04-27", "2026-04-30");
        assert_eq!(v, vec!["2026-04-27", "2026-04-28", "2026-04-29"]);
    }

    #[test]
    fn days_between_empty_when_from_eq_to() {
        assert!(days_between("2026-04-27", "2026-04-27").is_empty());
    }
}
