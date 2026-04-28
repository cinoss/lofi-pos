//! Tokio-based EOD scheduler.
//!
//! On startup: replays any business day strictly before today that still has
//! a `day_key` row but no `eod_runs.status='ok'` (catch-up).
//!
//! Steady state: sleeps until `next_cutoff_ms`, then runs the EOD pipeline
//! for the just-closed business day.
//!
//! NOTE: `state.settings` is an `Arc<Settings>` snapshot loaded at startup;
//! it is NOT refreshed when an Owner edits settings via /admin/settings.
//! Cutoff/tz changes therefore require an app restart to take effect on the
//! scheduler. (Adding interior mutability — `ArcSwap` or `RwLock` — was
//! intentionally deferred; see admin::update_settings.)

use crate::app_state::AppState;
use crate::eod::business_day::{business_day_for, next_cutoff_ms, Cfg};
use crate::eod::runner::run_eod;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

/// Hard cap on how many distinct business days `catch_up` will replay in a
/// single startup. A stale `day_key` row from years ago (e.g. dev-seeded data
/// or a clock that jumped) must not cause us to attempt to materialize report
/// files for thousands of mostly-empty days.
const CATCH_UP_MAX_DAYS: usize = 90;

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
///
/// Only days that actually appear in `day_key` (i.e. saw real activity) are
/// considered — empty calendar days never need a report file. A hard cap of
/// `CATCH_UP_MAX_DAYS` is applied to the candidate list, taking the most
/// recent days; a warning is logged if older days are skipped (which would
/// indicate a stale `day_key` row that should be cleaned up manually).
pub fn catch_up(state: &AppState) -> crate::error::AppResult<()> {
    let cfg = current_cfg(state);
    let today = business_day_for(state.clock.now_ms(), cfg);
    // NOTE: Post-UTC-rotation, `dek.utc_day` no longer corresponds 1:1 to
    // business_day. Catch-up still uses currently-held DEK days as a coarse
    // proxy for "days with potential activity" — any business_day not present
    // here will simply be skipped (its events have already been crypto-shred or
    // EOD-deleted). Acceptable until a dedicated business-day index is added.
    let active_days: Vec<String> = state
        .master
        .lock()
        .unwrap()
        .list_dek_days()?
        .into_iter()
        .map(|i| i.utc_day)
        .collect();

    // `list_active_business_days` returns only days with a wrapped DEK row.
    // Anything < today is a candidate. Sort ascending so we process oldest
    // first under the cap.
    let mut candidates: Vec<String> = active_days
        .into_iter()
        .filter(|d| d.as_str() < today.as_str())
        .collect();
    candidates.sort();

    if candidates.len() > CATCH_UP_MAX_DAYS {
        let skipped = candidates.len() - CATCH_UP_MAX_DAYS;
        let drop_to = candidates.len() - CATCH_UP_MAX_DAYS;
        let oldest_kept = candidates[drop_to].clone();
        let oldest_skipped = candidates[0].clone();
        tracing::warn!(
            skipped,
            oldest_skipped = %oldest_skipped,
            oldest_kept = %oldest_kept,
            cap = CATCH_UP_MAX_DAYS,
            "eod catch_up: too many backlogged days; processing only the most recent \
             CATCH_UP_MAX_DAYS. Older day_key rows likely need manual cleanup."
        );
        candidates.drain(..drop_to);
    }

    for day in candidates {
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
    fn catch_up_only_processes_days_present_in_day_key_skipping_empty_calendar_days() {
        // Today = 2026-04-30 12:00 +07. Seed a stale day_key for 2020-01-01
        // (years before today) plus a real order on 2026-04-27. The legacy
        // behavior (days_between(earliest, today)) would iterate >2,000
        // calendar days; the new behavior only iterates days that actually
        // have a day_key row.
        let rig = seed_app_state_at(2026, 4, 27, 7, 0, 0); // 14:00 +07 → BD 2026-04-27
        place_test_order(&rig);
        // Manually seed a stale wrapped DEK row — bypass crypto since the
        // value is opaque to catch_up (it just reads the day string), and an
        // empty-day run_eod still succeeds.
        rig.state
            .master
            .lock()
            .unwrap()
            .put_dek("2020-01-01", &[0u8; 32], 0)
            .unwrap();
        // Advance to 2026-04-30 12:00 +07 (07:00 UTC) — +71h from 07:00 UTC
        // on 2026-04-27.
        rig.clock.advance_minutes(71 * 60 - 2 * 60); // → 2026-04-30 04:00 UTC
        rig.clock.advance_minutes(60); // → 05:00 UTC = 12:00 +07

        catch_up(&rig.state).unwrap();

        // 2026-04-27 had real activity → ran successfully.
        assert_eq!(eod_runs_status(&rig.state, "2026-04-27"), "ok");
        // 2020-01-01 was in day_key → processed too (empty-day run is OK).
        assert_eq!(eod_runs_status(&rig.state, "2020-01-01"), "ok");
        // Days that are NOT in day_key (the >2,000 calendar days between
        // 2020-01-01 and 2026-04-27, plus 2026-04-28/29) must NOT have
        // eod_runs rows — the legacy iteration bug would have created them.
        assert_eq!(eod_runs_status(&rig.state, "2020-01-02"), "");
        assert_eq!(eod_runs_status(&rig.state, "2023-06-15"), "");
        assert_eq!(eod_runs_status(&rig.state, "2026-04-28"), "");
        assert_eq!(eod_runs_status(&rig.state, "2026-04-29"), "");
    }

    #[test]
    fn catch_up_caps_at_max_days_dropping_oldest() {
        // Seed 95 candidate day_key rows (older than today). With the cap
        // at CATCH_UP_MAX_DAYS=90, the 5 oldest must be skipped.
        let rig = seed_app_state_at(2026, 4, 30, 7, 0, 0); // 14:00 +07 → BD 2026-04-30
        // Use synthetic ISO dates well before today. Pick 95 distinct days
        // in the year 2024 (Jan–Apr roughly).
        let mut seeded: Vec<String> = Vec::new();
        for i in 0..95 {
            let day = format!("2024-{:02}-{:02}", 1 + (i / 28) as u32, 1 + (i % 28) as u32);
            rig.state
                .master
                .lock()
                .unwrap()
                .put_dek(&day, &[0u8; 32], 0)
                .unwrap();
            seeded.push(day);
        }
        seeded.sort();

        catch_up(&rig.state).unwrap();

        // Five oldest must be skipped (no eod_runs row).
        for day in &seeded[..5] {
            assert_eq!(
                eod_runs_status(&rig.state, day),
                "",
                "{day} should be skipped by cap"
            );
        }
        // Newest must be processed.
        assert_eq!(eod_runs_status(&rig.state, &seeded[94]), "ok");
        assert_eq!(eod_runs_status(&rig.state, &seeded[5]), "ok");
    }

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
