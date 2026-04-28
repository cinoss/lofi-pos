use chrono::{DateTime, Datelike, Duration, FixedOffset, Utc};

/// Business day spans `cutoff_hour` (LOCAL time) to `cutoff_hour` next day.
/// `tz` is the venue's fixed offset from UTC (e.g. `FixedOffset::east_opt(7*3600).unwrap()` for Vietnam).
/// An event at UTC time `t` belongs to business day = (local(t) - cutoff_hour hours).date.
pub fn business_day_of(t: DateTime<Utc>, tz: FixedOffset, cutoff_hour: u32) -> String {
    let local = t.with_timezone(&tz);
    let shifted = local - Duration::hours(cutoff_hour as i64);
    format!(
        "{:04}-{:02}-{:02}",
        shifted.year(),
        shifted.month(),
        shifted.day()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[test]
    fn before_cutoff_belongs_to_previous_day() {
        let utc = FixedOffset::east_opt(0).unwrap();
        // 2026-04-28 03:00 with cutoff 11 → previous day (2026-04-27)
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 3, 0), utc, 11),
            "2026-04-27"
        );
    }

    #[test]
    fn at_cutoff_belongs_to_new_day() {
        let utc = FixedOffset::east_opt(0).unwrap();
        // 2026-04-28 11:00 with cutoff 11 → 2026-04-28 (boundary inclusive)
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 11, 0), utc, 11),
            "2026-04-28"
        );
    }

    #[test]
    fn after_cutoff_belongs_to_new_day() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 12, 0), utc, 11),
            "2026-04-28"
        );
    }

    #[test]
    fn midnight_with_cutoff_11_is_previous_day() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 0, 0), utc, 11),
            "2026-04-27"
        );
    }

    #[test]
    fn midnight_with_cutoff_0_is_same_day() {
        let utc = FixedOffset::east_opt(0).unwrap();
        assert_eq!(business_day_of(dt(2026, 4, 28, 0, 0), utc, 0), "2026-04-28");
    }

    #[test]
    fn cutoff_22_late_night() {
        let utc = FixedOffset::east_opt(0).unwrap();
        // 2026-04-28 21:00 with cutoff 22 → still 2026-04-27
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 21, 0), utc, 22),
            "2026-04-27"
        );
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 22, 0), utc, 22),
            "2026-04-28"
        );
    }

    #[test]
    fn vietnam_local_cutoff_works_correctly() {
        let vn = FixedOffset::east_opt(7 * 3600).unwrap();
        // Local 2026-04-28 10:59 (= UTC 03:59) with cutoff 11 → still 2026-04-27
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 3, 59), vn, 11),
            "2026-04-27"
        );
        // Local 2026-04-28 11:00 (= UTC 04:00) with cutoff 11 → 2026-04-28
        assert_eq!(business_day_of(dt(2026, 4, 28, 4, 0), vn, 11), "2026-04-28");
        // Local 2026-04-28 18:00 (= UTC 11:00) with cutoff 11 → 2026-04-28 (NOT next day, this is the bug we're preventing)
        assert_eq!(
            business_day_of(dt(2026, 4, 28, 11, 0), vn, 11),
            "2026-04-28"
        );
    }
}
