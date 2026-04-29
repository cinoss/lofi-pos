//! Single transactional EOD run for one business day.
//!
//! Sequence:
//!   1. Idempotency check: if `eod_runs.status='ok'` for the day, no-op.
//!   2. Mark `eod_runs.status='running'`.
//!   3. Build the report (decrypt every event for the day).
//!   4. POST the report to the bouncer (`/reports/eod`). On failure: mark
//!      `eod_runs.status='failed'` and bail; event rows are NOT deleted so
//!      a later catch-up can retry.
//!   5. Master tx: prune old idempotency + expired denylist tokens, mark
//!      `eod_runs.status='ok'`.
//!   6. Delete `event` rows for `business_day` from events.db.
//!
//! Local `daily_report` table and `reports/<day>.json` files are gone — the
//! bouncer owns historical report storage off-box.

use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::eod::builder::build_report;
use rusqlite::params;

#[derive(Debug, Clone)]
pub struct RunResult {
    pub business_day: String,
    pub status: &'static str,
}

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

    // 3) Build report.
    let report = match build_report(state, business_day) {
        Ok(r) => r,
        Err(e) => {
            if let Err(me) = mark_failed(state, business_day, &e.to_string()) {
                tracing::error!(day = %business_day, err = %me, "eod mark_failed failed");
            }
            return Err(e);
        }
    };

    // 4) POST report to bouncer. Failure => mark failed and bail; do NOT
    //    delete event rows.
    let payload = serde_json::to_value(&report)
        .map_err(|e| AppError::Internal(format!("report to_value: {e}")))?;
    if let Err(e) = state
        .bouncer
        .post_report(business_day, state.clock.now_ms(), &payload)
    {
        if let Err(me) = mark_failed(state, business_day, &e.to_string()) {
            tracing::error!(day = %business_day, err = %me, "eod mark_failed failed");
        }
        return Err(e);
    }

    // 5) Atomic db updates.
    let finished_at = state.clock.now_ms();
    {
        let mut master = state.master.lock().unwrap();
        master.with_tx(|tx| {
            // Prune idempotency rows older than ~1 day.
            tx.execute(
                "DELETE FROM idempotency_key WHERE created_at < ?1",
                params![finished_at - 24 * 3600 * 1000],
            )?;
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

    // 6) Delete event rows for this business day.
    if let Err(e) = state.events.delete_day(business_day) {
        tracing::error!(
            day = %business_day,
            err = %e,
            "eod: event-row delete failed; report already POSTed"
        );
    }

    Ok(RunResult {
        business_day: business_day.to_string(),
        status: "ok",
    })
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
    fn run_marks_eod_runs_ok_and_deletes_event_rows() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        assert!(rig.state.events.count_for_day("2026-04-27").unwrap() > 0);

        let result = run_eod(&rig.state, "2026-04-27").unwrap();
        assert_eq!(result.status, "ok");
        assert_eq!(rig.state.events.count_for_day("2026-04-27").unwrap(), 0);
        assert_eq!(eod_runs_status(&rig.state, "2026-04-27"), "ok");
    }

    #[test]
    fn run_idempotent_second_call_is_noop() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        run_eod(&rig.state, "2026-04-27").unwrap();
        let again = run_eod(&rig.state, "2026-04-27").unwrap();
        assert_eq!(again.status, "ok");
        assert_eq!(rig.state.events.count_for_day("2026-04-27").unwrap(), 0);
    }

    #[test]
    fn run_prunes_old_idempotency_keys() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        let stale_ts = rig.clock.now_ms() - 48 * 3600 * 1000;
        insert_idempotency(&rig.state, "stale-key", stale_ts);
        assert!(idempotency_exists(&rig.state, "stale-key"));
        run_eod(&rig.state, "2026-04-27").unwrap();
        assert!(!idempotency_exists(&rig.state, "stale-key"));
    }

    #[test]
    fn run_empty_day_still_succeeds() {
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0);
        let r = run_eod(&rig.state, "2026-04-27").unwrap();
        assert_eq!(r.status, "ok");
        assert_eq!(eod_runs_status(&rig.state, "2026-04-27"), "ok");
    }

    #[test]
    fn run_marks_failed_when_bouncer_unreachable_and_keeps_event_rows() {
        // The default test_support rig builds a BouncerClient pointing at an
        // unreachable port (127.0.0.1:1) — see test_support::seed_app_state_at.
        // So `post_report` will fail and the runner must NOT delete events.
        let rig = seed_app_state_at_failing_bouncer(2026, 4, 27, 7, 0, 0);
        place_test_order(&rig);
        let before = rig.state.events.count_for_day("2026-04-27").unwrap();
        let res = run_eod(&rig.state, "2026-04-27");
        assert!(res.is_err(), "expected bouncer post to fail");
        assert_eq!(eod_runs_status(&rig.state, "2026-04-27"), "failed");
        // Event rows preserved for retry.
        assert_eq!(
            rig.state.events.count_for_day("2026-04-27").unwrap(),
            before
        );
    }
}
