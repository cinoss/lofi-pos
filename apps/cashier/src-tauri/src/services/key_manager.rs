//! UTC-day-keyed Data Encryption Key manager.
//!
//! Replaces the old `services::day_key` helper. Two responsibilities:
//!
//! 1. **Lookup / lazy-create today's DEK** for the encrypt path. The on-disk
//!    representation is a wrapped DEK in `master.dek` keyed by UTC calendar
//!    day. First write of the day generates a fresh random DEK, wraps it under
//!    the KEK, and persists; subsequent writes hit the existing row.
//! 2. **Rotation** — invoked by `rotation::scheduler` at every UTC midnight.
//!    Ensures today's DEK exists (catch-up after restart) and prunes any DEK
//!    older than `KEY_TTL_DAYS` UTC days, providing the crypto-shred guarantee
//!    independently of the EOD pipeline.

use crate::crypto::{Dek, Kek};
use crate::error::{AppError, AppResult};
use crate::services::utc_day::{days_ago, utc_day_of};
use crate::store::master::Master;
use std::sync::{Arc, Mutex};

/// Hard upper bound on DEK age. Events encrypted with a DEK older than this
/// become unreadable once `rotate()` runs (key crypto-shred).
pub const KEY_TTL_DAYS: i64 = 3;

/// UTC-day key manager.
///
/// Cheap to clone (everything behind `Arc`). Hold via `Arc<KeyManager>` in
/// `AppState` and pass into `EventService` and the rotation scheduler.
pub struct KeyManager {
    master: Arc<Mutex<Master>>,
    kek: Arc<Kek>,
}

/// Outcome of a single `rotate()` invocation. Passed to the tracing layer so
/// ops can see what the scheduler actually did at each tick.
#[derive(Debug)]
pub struct RotationReport {
    pub today: String,
    pub created_today: bool,
    pub deleted: Vec<String>,
}

impl KeyManager {
    pub fn new(master: Arc<Mutex<Master>>, kek: Arc<Kek>) -> Self {
        Self { master, kek }
    }

    /// Return the DEK for `utc_day_of(now_ms)`. Lazily creates and persists
    /// the row on the very first write of a UTC day.
    pub fn current_dek(&self, now_ms: i64) -> AppResult<Dek> {
        let day = utc_day_of(now_ms);
        self.get_or_create(&day, now_ms)
    }

    /// Look up the DEK persisted for `utc_day`. Returns `Crypto("key
    /// expired …")` if no row exists — the read path treats missing-key the
    /// same as a tampered ciphertext.
    pub fn dek_for(&self, utc_day: &str) -> AppResult<Dek> {
        let m = self.master.lock().unwrap();
        let wrapped = m
            .get_dek(utc_day)?
            .ok_or_else(|| AppError::Crypto(format!("key expired for {utc_day}")))?;
        self.kek.unwrap(&wrapped)
    }

    /// One rotation pass:
    ///   1. Ensure today's DEK exists (catch-up at startup or across midnight).
    ///   2. Delete any DEK whose `utc_day < today − KEY_TTL_DAYS`.
    ///
    /// Idempotent: a second consecutive call on the same UTC day reports no
    /// new creation and no deletions.
    pub fn rotate(&self, now_ms: i64) -> AppResult<RotationReport> {
        let today = utc_day_of(now_ms);
        let created_today = self.ensure_dek_inserted(&today, now_ms)?;
        let oldest_keep = days_ago(&today, KEY_TTL_DAYS);
        let deleted = {
            let m = self.master.lock().unwrap();
            m.delete_deks_older_than(&oldest_keep)?
        };
        Ok(RotationReport {
            today,
            created_today,
            deleted,
        })
    }

    fn get_or_create(&self, utc_day: &str, now_ms: i64) -> AppResult<Dek> {
        let m = self.master.lock().unwrap();
        if let Some(wrapped) = m.get_dek(utc_day)? {
            return self.kek.unwrap(&wrapped);
        }
        let dek = Dek::new_random();
        let wrapped = self.kek.wrap(&dek)?;
        let inserted = m.put_dek(utc_day, &wrapped, now_ms)?;
        if inserted {
            Ok(dek)
        } else {
            // Lost the race; another writer beat us. Read theirs.
            let stored = m.get_dek(utc_day)?.ok_or(AppError::NotFound)?;
            self.kek.unwrap(&stored)
        }
    }

    /// Like `get_or_create` but returns whether a row was newly inserted, so
    /// `rotate()` can report `created_today = true/false` without leaking the
    /// DEK material.
    fn ensure_dek_inserted(&self, utc_day: &str, now_ms: i64) -> AppResult<bool> {
        let m = self.master.lock().unwrap();
        if m.get_dek(utc_day)?.is_some() {
            return Ok(false);
        }
        let dek = Dek::new_random();
        let wrapped = self.kek.wrap(&dek)?;
        m.put_dek(utc_day, &wrapped, now_ms)
    }

    /// Test-only seam: create a DEK at an explicit historical `utc_day`. Used
    /// by tests that need to pre-populate "old" rows so `rotate()` has
    /// something to prune.
    #[cfg(test)]
    pub fn current_dek_at(&self, utc_day: &str, now_ms: i64) -> AppResult<Dek> {
        self.get_or_create(utc_day, now_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Kek;
    use crate::store::master::Master;
    use std::sync::{Arc, Mutex};

    fn rig() -> KeyManager {
        let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
        let kek = Arc::new(Kek::new_random());
        KeyManager::new(master, kek)
    }

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Utc
            .with_ymd_and_hms(y, m, d, 12, 0, 0)
            .unwrap()
            .timestamp_millis()
    }

    #[test]
    fn current_dek_creates_on_first_call() {
        let km = rig();
        let d1 = km.current_dek(ts(2026, 4, 28)).unwrap();
        let d2 = km.current_dek(ts(2026, 4, 28)).unwrap();
        assert_eq!(d1.as_bytes(), d2.as_bytes());
    }

    #[test]
    fn current_dek_differs_per_utc_day() {
        let km = rig();
        let d1 = km.current_dek(ts(2026, 4, 28)).unwrap();
        let d2 = km.current_dek(ts(2026, 4, 29)).unwrap();
        assert_ne!(d1.as_bytes(), d2.as_bytes());
    }

    #[test]
    fn dek_for_returns_key_expired_if_missing() {
        let km = rig();
        let res = km.dek_for("1999-01-01");
        match res {
            Err(AppError::Crypto(msg)) => assert!(msg.contains("key expired")),
            Err(e) => panic!("expected Crypto(\"key expired\"), got {e}"),
            Ok(_) => panic!("expected error, got Ok"),
        }
    }

    #[test]
    fn rotate_creates_today_and_prunes_older_than_3_days() {
        let km = rig();
        // Pre-seed 5 days of historic keys. Today = 2026-04-28; with
        // KEY_TTL_DAYS=3, oldest_keep = 2026-04-25, so 22/23/24 are pruned.
        for d in &[
            "2026-04-22",
            "2026-04-23",
            "2026-04-24",
            "2026-04-25",
            "2026-04-26",
        ] {
            km.current_dek_at(d, ts(2026, 4, 22)).unwrap();
        }
        let report = km.rotate(ts(2026, 4, 28)).unwrap();
        assert_eq!(report.deleted, vec!["2026-04-22", "2026-04-23", "2026-04-24"]);
        assert!(report.created_today);
    }

    #[test]
    fn rotate_idempotent() {
        let km = rig();
        let r1 = km.rotate(ts(2026, 4, 28)).unwrap();
        let r2 = km.rotate(ts(2026, 4, 28)).unwrap();
        assert!(r1.created_today);
        assert!(!r2.created_today);
        assert!(r2.deleted.is_empty());
    }
}
