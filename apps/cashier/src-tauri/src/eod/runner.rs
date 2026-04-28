//! Single transactional EOD run for one business day.
//!
//! Sequence:
//!   1. Idempotency check: if `eod_runs.status='ok'` for the day, no-op.
//!   2. Mark `eod_runs.status='running'`.
//!   3. Build the report (decrypt every event for the day).
//!   4. Write report JSON to `<reports_dir>/<day>.json`.
//!   5. In a single tx: insert/replace `daily_report`, delete `day_key`
//!      (crypto-shred), prune old `idempotency_key` + expired
//!      `token_denylist`, mark `eod_runs.status='ok'`.
//!
//! After step 5 the events for that day are unreadable on disk: the wrapped
//! DEK is gone and AES-256-GCM ciphertext without its key is irrecoverable.
//! The `daily_report` row is the durable record.

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::eod::builder::build_report;
use rusqlite::params;
use std::fs;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub business_day: String,
    pub status: &'static str,
}

/// Run the EOD pipeline for `business_day`.
///
/// Idempotent: if the day already completed (`eod_runs.status='ok'`) the
/// function returns immediately without re-doing the work. Failed prior
/// attempts are retried.
pub fn run_eod(state: &AppState, business_day: &str) -> AppResult<RunResult> {
    // 1) Already-done short-circuit.
    let already = state
        .master
        .lock()
        .unwrap()
        .get_eod_runs_status(business_day)?;
    if already.as_deref() == Some("ok") {
        return Ok(RunResult {
            business_day: business_day.to_string(),
            status: "ok",
        });
    }

    // 2) Mark running.
    let started_at = state.clock.now_ms();
    {
        let conn = state.master.lock().unwrap();
        conn.upsert_eod_running(business_day, started_at)?;
    }

    // 3) Build report. On failure, mark eod_runs.status='failed' and bail.
    let report = match build_report(state, business_day) {
        Ok(r) => r,
        Err(e) => {
            if let Err(me) = mark_failed(state, business_day, &e.to_string()) {
                tracing::error!(day = %business_day, err = %me, "eod mark_failed failed");
            }
            return Err(e);
        }
    };

    // 4) Write report file. (Treated as fatal; if it fails the day stays
    //    'failed' so the next catch-up retries.)
    if let Err(e) = write_report_file(state, &report) {
        if let Err(me) = mark_failed(state, business_day, &e.to_string()) {
            tracing::error!(day = %business_day, err = %me, "eod mark_failed failed");
        }
        return Err(e);
    }

    // 5) Atomic db updates.
    let order_summary = serde_json::to_string(&report)
        .map_err(|e| AppError::Internal(format!("report serialize: {e}")))?;
    let finished_at = state.clock.now_ms();
    {
        let mut master = state.master.lock().unwrap();
        master.with_tx(|tx| {
            tx.execute(
                "INSERT OR REPLACE INTO daily_report
                 (business_day, generated_at, order_summary_json, inventory_summary_json)
                 VALUES (?1, ?2, ?3, '{}')",
                params![business_day, started_at, order_summary],
            )?;
            // Crypto-shred: drop the wrapped DEK. Events for this day are now
            // irrecoverable (AES-256-GCM with the AAD bound to key_id).
            tx.execute(
                "DELETE FROM day_key WHERE business_day = ?1",
                params![business_day],
            )?;
            // Prune idempotency rows older than ~1 day. We don't store
            // business_day on idempotency_key (see master::list_idempotency_for_day),
            // so the bound is by `created_at` ms.
            tx.execute(
                "DELETE FROM idempotency_key WHERE created_at < ?1",
                params![finished_at - 24 * 3600 * 1000],
            )?;
            // Prune any tokens whose exp has passed.
            tx.execute(
                "DELETE FROM token_denylist WHERE expires_at < ?1",
                params![finished_at],
            )?;
            tx.execute(
                "UPDATE eod_runs SET finished_at = ?1, status = 'ok', error = NULL
                 WHERE business_day = ?2",
                params![finished_at, business_day],
            )?;
            Ok(())
        })?;
    }

    Ok(RunResult {
        business_day: business_day.to_string(),
        status: "ok",
    })
}

fn write_report_file(state: &AppState, report: &crate::eod::builder::Report) -> AppResult<()> {
    fs::create_dir_all(&state.reports_dir).map_err(AppError::Io)?;
    let path = state
        .reports_dir
        .join(format!("{}.json", report.business_day));
    let bytes = serde_json::to_vec_pretty(report)
        .map_err(|e| AppError::Internal(format!("report serialize: {e}")))?;
    fs::write(&path, bytes).map_err(AppError::Io)?;
    Ok(())
}

fn mark_failed(state: &AppState, day: &str, err: &str) -> AppResult<()> {
    let now = state.clock.now_ms();
    let conn = state.master.lock().unwrap();
    conn.set_eod_runs_failed(day, now, err)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eod::test_support::*;
    use crate::time::Clock;

    #[test]
    fn run_marks_eod_runs_ok_writes_report_deletes_day_key() {
        // 2026-04-27 14:00 +07 → business day 2026-04-27.
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        // First event for the day created the wrapped DEK row.
        assert!(day_key_exists(&rig.state, "2026-04-27"));

        let result = run_eod(&rig.state, "2026-04-27").unwrap();
        assert_eq!(result.status, "ok");
        assert!(daily_report_exists(&rig.state, "2026-04-27"));
        assert!(rig.state.reports_dir.join("2026-04-27.json").exists());
        // Crypto-shred: day_key gone.
        assert!(!day_key_exists(&rig.state, "2026-04-27"));
        assert_eq!(eod_runs_status(&rig.state, "2026-04-27"), "ok");
    }

    #[test]
    fn run_idempotent_second_call_is_noop() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        run_eod(&rig.state, "2026-04-27").unwrap();
        // Second call short-circuits on status='ok' without retrying any work.
        let again = run_eod(&rig.state, "2026-04-27").unwrap();
        assert_eq!(again.status, "ok");
        // Still no day_key row (no double-shred attempt).
        assert!(!day_key_exists(&rig.state, "2026-04-27"));
    }

    #[test]
    fn run_prunes_old_idempotency_keys() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        // Insert a synthetic stale idempotency row whose created_at is
        // older than 24h vs. the runner's clock.
        let stale_ts = rig.clock.now_ms() - 48 * 3600 * 1000;
        insert_idempotency(&rig.state, "stale-key", stale_ts);
        assert!(idempotency_exists(&rig.state, "stale-key"));
        run_eod(&rig.state, "2026-04-27").unwrap();
        assert!(!idempotency_exists(&rig.state, "stale-key"));
    }

    #[test]
    fn run_empty_day_still_succeeds() {
        // No events at all for the day — pipeline must still produce a
        // (empty) report row and mark eod_runs ok.
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        let r = run_eod(&rig.state, "2026-04-27").unwrap();
        assert_eq!(r.status, "ok");
        assert!(daily_report_exists(&rig.state, "2026-04-27"));
        assert!(rig.state.reports_dir.join("2026-04-27.json").exists());
    }
}

