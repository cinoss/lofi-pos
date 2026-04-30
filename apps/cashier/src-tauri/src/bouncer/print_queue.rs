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
    // Tauri setup runs sync; use tauri::async_runtime so this works from there.
    tauri::async_runtime::spawn(async move {
        loop {
            let next = {
                let m = state.master.lock().unwrap();
                m.next_print_job(state.clock.now_ms())
            };
            match next {
                Ok(Some(job)) => {
                    let job_id = job.id;
                    let attempts = job.attempts;
                    // Malformed JSON in the queue row means this print is
                    // unrecoverable — sending `null` to the bouncer would
                    // silently lose the artifact. Drop the row and surface
                    // the error in the audit log.
                    let payload: serde_json::Value =
                        match serde_json::from_str(&job.payload_json) {
                            Ok(v) => v,
                            Err(e) => {
                                tracing::error!(
                                    job_id,
                                    error = %e,
                                    payload_preview = %truncate(&job.payload_json, 200),
                                    "print queue: malformed payload_json; dropping job (data loss for this print)"
                                );
                                if let Err(de) =
                                    state.master.lock().unwrap().delete_print_job(job_id)
                                {
                                    tracing::error!(?de, job_id, "print queue: delete of malformed job failed");
                                }
                                continue;
                            }
                        };
                    // The bouncer client's `print` method is async and
                    // internally offloads the blocking reqwest call to
                    // `spawn_blocking`, so we can await it directly.
                    let result = state
                        .bouncer
                        .print(&job.kind, &payload, job.target.as_deref())
                        .await;
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

fn truncate(s: &str, max: usize) -> &str {
    if s.len() <= max {
        s
    } else {
        // Avoid splitting a UTF-8 codepoint; back off to a char boundary.
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        &s[..end]
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

    #[test]
    fn truncate_handles_utf8_boundary() {
        let s = "héllo";
        // limit at 2 lands inside the é (2-byte char); truncate must not panic.
        assert!(truncate(s, 2).len() <= 2);
    }
}
