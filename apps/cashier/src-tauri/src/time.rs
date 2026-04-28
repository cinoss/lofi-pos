use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn now_ms(&self) -> i64 {
        self.now().timestamp_millis()
    }
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

// NOTE: not gated by #[cfg(test)] — integration tests in `tests/` need access,
// and Cargo's cfg(test) doesn't apply to the lib when compiling integration tests.
// Adds ~few hundred bytes to release binary, no secrets exposed.
pub mod test_support {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Mutex;

    pub struct MockClock(Mutex<DateTime<Utc>>);

    impl MockClock {
        pub fn new(t: DateTime<Utc>) -> Self {
            Self(Mutex::new(t))
        }
        pub fn at_ymd_hms(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> Self {
            Self::new(Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap())
        }
        pub fn set(&self, t: DateTime<Utc>) {
            *self.0.lock().unwrap() = t;
        }
        pub fn advance_minutes(&self, n: i64) {
            let mut g = self.0.lock().unwrap();
            *g += chrono::Duration::minutes(n);
        }
    }
    impl Clock for MockClock {
        fn now(&self) -> DateTime<Utc> {
            *self.0.lock().unwrap()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::MockClock;

    #[test]
    fn system_clock_now_ms_increases() {
        let c = SystemClock;
        let a = c.now_ms();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = c.now_ms();
        assert!(b > a);
    }

    #[test]
    fn mock_clock_returns_set_time() {
        let c = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
        // 2026-04-27T12:00:00Z epoch seconds (deviates from plan's 1777_896_000,
        // which corresponds to 2026-05-04T12:00:00Z; plan value was off by 7 days).
        assert_eq!(c.now().timestamp(), 1_777_291_200);
    }

    #[test]
    fn mock_clock_advance() {
        let c = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
        c.advance_minutes(30);
        // 1_777_291_200 + 30*60 = 1_777_293_000 (plan's 1777_897_800 was based on the
        // same off-by-7-days base value).
        assert_eq!(c.now().timestamp(), 1_777_293_000);
    }
}
