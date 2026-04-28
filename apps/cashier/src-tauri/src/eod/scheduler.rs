//! Tokio-based EOD scheduler.
//!
//! On startup: replays any business day strictly before today that still has
//! a `day_key` row but no `eod_runs.status='ok'` (catch-up).
//!
//! Steady state: sleeps until `next_cutoff_ms`, then runs the EOD pipeline
//! for the just-closed business day. Re-reads the cutoff/tz settings every
//! tick so an Owner-edit takes effect on the next iteration.

use crate::app_state::AppState;
use crate::eod::business_day::{business_day_for, days_between, next_cutoff_ms, Cfg};
use crate::eod::runner::run_eod;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

/// Spawn the EOD scheduler on the current Tokio runtime.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        if let Err(e) = catch_up(&state) {
            tracing::error!(?e, "eod catch-up failed");
        }
        loop {
            let cfg = current_cfg(&state);
            let now = state.clock.now_ms();
            let next = next_cutoff_ms(now, cfg);
            // `next - now` is positive by construction; clamp to >= 1s for
            // sanity and so we never busy-loop on a clock that jumped.
            let wait_ms = (next - now).max(1_000) as u64;
            tracing::info!(wait_ms, "eod scheduler sleeping until next cutoff");
            sleep(Duration::from_millis(wait_ms)).await;

            // Right after the cutoff fires, the just-closed business day is
            // the one for `now - 1ms`. We re-read the cfg in case settings
            // were edited mid-sleep.
            let cfg2 = current_cfg(&state);
            let now2 = state.clock.now_ms();
            let just_closed = business_day_for(now2 - 1000, cfg2);
            if let Err(e) = run_eod(&state, &just_closed) {
                tracing::error!(day = %just_closed, ?e, "eod run failed");
            }
        }
    });
}

/// Process every business day strictly before today that still has a
/// wrapped DEK (i.e. events on disk) but no successful `eod_runs` row.
pub fn catch_up(state: &AppState) -> crate::error::AppResult<()> {
    let cfg = current_cfg(state);
    let today = business_day_for(state.clock.now_ms(), cfg);
    let active_days = state.master.lock().unwrap().list_active_business_days()?;
    // `list_active_business_days` returns only days with a wrapped DEK row.
    // Anything < today is a candidate.
    let earliest = match active_days.iter().find(|d| d.as_str() < today.as_str()) {
        Some(d) => d.clone(),
        None => return Ok(()),
    };
    for day in days_between(&earliest, &today) {
        let already = state.master.lock().unwrap().get_eod_runs_status(&day)?;
        if already.as_deref() == Some("ok") {
            continue;
        }
        if let Err(e) = run_eod(state, &day) {
            tracing::error!(day = %day, ?e, "catch-up failed");
        }
    }
    Ok(())
}

fn current_cfg(state: &AppState) -> Cfg {
    let s = &state.settings;
    Cfg {
        cutoff_hour: s.business_day_cutoff_hour,
        tz_offset_seconds: s.business_day_tz.local_minus_utc(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eod::test_support::*;

    #[test]
    fn catch_up_processes_old_unprocessed_days() {
        // Frozen clock at 2026-04-30 12:00 +07 = today's BD = 2026-04-30.
        // We seed events on 2026-04-27 and 2026-04-28 by advancing through
        // those days while writing.
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0); // 14:00 +07 → BD 2026-04-27
        place_test_order(&rig);
        // Advance to 2026-04-28 14:00 +07 (07:00 UTC + 24h = next day same time).
        rig.clock.advance_minutes(24 * 60);
        place_test_order(&rig);
        // Advance to 2026-04-30 12:00 +07 = 2026-04-30 05:00 UTC. From
        // current 2026-04-28 07:00 UTC that is +47h.
        rig.clock.advance_minutes(47 * 60 - 2 * 60); // =45h → 2026-04-30 04:00 UTC = 11:00 +07
        rig.clock.advance_minutes(60); // → 12:00 +07
        catch_up(&rig.state).unwrap();
        assert_eq!(eod_runs_status(&rig.state, "2026-04-27"), "ok");
        assert_eq!(eod_runs_status(&rig.state, "2026-04-28"), "ok");
    }
}
