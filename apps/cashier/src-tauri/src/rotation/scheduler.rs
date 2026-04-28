//! Tokio-based UTC key rotation scheduler.
//!
//! On startup: invokes `KeyManager::rotate` once for catch-up — this both
//! ensures today's DEK exists (relevant after a restart that spans a UTC
//! midnight) and prunes any DEK older than `KEY_TTL_DAYS`.
//!
//! Steady state: sleeps until the next UTC midnight, then runs `rotate` again.

use crate::app_state::AppState;
use crate::services::utc_day::next_utc_midnight_ms;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

/// Spawn the rotation task on the current Tokio runtime.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Catch-up at startup.
        match state.key_manager.rotate(state.clock.now_ms()) {
            Ok(r) => tracing::info!(
                today = %r.today,
                created = r.created_today,
                deleted = ?r.deleted,
                "key rotation startup"
            ),
            Err(e) => tracing::error!(?e, "key rotation startup failed"),
        }
        loop {
            let now = state.clock.now_ms();
            let next = next_utc_midnight_ms(now);
            // Clamp to >= 1s in case the clock jumped right onto midnight.
            let wait_ms = (next - now).max(1_000) as u64;
            tracing::info!(wait_ms, "key rotation sleeping until next UTC midnight");
            sleep(Duration::from_millis(wait_ms)).await;
            match state.key_manager.rotate(state.clock.now_ms()) {
                Ok(r) => tracing::info!(
                    today = %r.today,
                    created = r.created_today,
                    deleted = ?r.deleted,
                    "key rotation"
                ),
                Err(e) => tracing::error!(?e, "key rotation failed"),
            }
        }
    });
}
