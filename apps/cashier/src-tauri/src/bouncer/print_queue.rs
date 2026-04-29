//! Background print queue worker.
//!
//! On every domain event that triggers a printable artifact, the command
//! pipeline calls [`crate::print::enqueue`] which inserts a row into
//! `master.print_queue`. A single tokio task drains FIFO, POSTs each job to
//! the bouncer, and either deletes the row on success or reschedules with
//! exponential backoff on failure.

use crate::app_state::AppState;
use std::sync::Arc;
use std::time::Duration;

/// Spawn the print-queue worker on the current Tokio runtime. The worker
/// runs forever; the caller is expected to hold the JoinHandle implicitly
/// via `tokio::spawn`.
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let next = {
                let m = state.master.lock().unwrap();
                m.next_print_job(state.clock.now_ms())
            };
            match next {
                Ok(Some(job)) => {
                    let payload: serde_json::Value =
                        serde_json::from_str(&job.payload_json).unwrap_or(serde_json::Value::Null);
                    let bouncer = state.bouncer.clone();
                    let target = job.target.clone();
                    let kind = job.kind.clone();
                    let job_id = job.id;
                    let attempts = job.attempts;
                    // The blocking reqwest call goes through spawn_blocking so
                    // we don't stall the runtime if the bouncer is slow.
                    let result = tokio::task::spawn_blocking(move || {
                        bouncer.print(&kind, &payload, target.as_deref())
                    })
                    .await
                    .unwrap_or_else(|e| {
                        Err(crate::error::AppError::Internal(format!("join: {e}")))
                    });
                    match result {
                        Ok(()) => {
                            if let Err(e) = state.master.lock().unwrap().delete_print_job(job_id) {
                                tracing::error!(?e, "print queue: delete after success failed");
                            }
                        }
                        Err(e) => {
                            let backoff_ms = backoff_for(attempts);
                            let next_try_at = state.clock.now_ms() + backoff_ms;
                            tracing::warn!(job_id, attempts, ?e, backoff_ms, "print failed; retrying");
                            if let Err(re) = state.master.lock().unwrap().reschedule_print_job(
                                job_id,
                                &e.to_string(),
                                next_try_at,
                            ) {
                                tracing::error!(?re, "print queue: reschedule failed");
                            }
                            // Pause briefly to avoid a hot loop if many jobs
                            // are failing concurrently.
                            tokio::time::sleep(Duration::from_millis(250)).await;
                        }
                    }
                }
                Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(e) => {
                    tracing::error!(?e, "print queue read failed");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

pub fn backoff_for(attempts: i64) -> i64 {
    match attempts {
        0 => 1_000,
        1 => 5_000,
        2 => 15_000,
        3 => 60_000,
        _ => 300_000,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_climbs_with_attempts() {
        assert!(backoff_for(0) < backoff_for(1));
        assert!(backoff_for(1) < backoff_for(2));
        assert!(backoff_for(2) < backoff_for(3));
        assert_eq!(backoff_for(4), backoff_for(99));
    }
}
