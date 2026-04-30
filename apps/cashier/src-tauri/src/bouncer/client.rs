//! HTTP client for the bouncer sidecar (localhost:7879 by default).
//!
//! Wire protocol per `docs/superpowers/specs/2026-04-29-bouncer-sidecar-design.md`.
//!
//! The underlying transport is `reqwest::blocking`, which internally owns a
//! tokio runtime. To keep callers safe by construction we expose two
//! flavours of every request:
//!
//!   * `*_blocking` — the raw blocking call. ONLY safe to invoke from a
//!     pure-sync context (process startup, CLI entry points, tests with no
//!     ambient tokio runtime).
//!   * the same name without the suffix — an `async fn` that offloads the
//!     blocking call to `tokio::task::spawn_blocking`. This is the form
//!     async tasks (the EOD scheduler, the print-queue worker, axum
//!     handlers) MUST use; calling the `*_blocking` variant from inside a
//!     tokio task can panic on the inner reqwest runtime.
//!
//! The inner `reqwest::blocking::Client` is built eagerly in
//! [`BouncerClient::new`] (which runs at app startup, before any tokio task
//! is dispatched) so the lazy-init code path that would otherwise allocate
//! an internal runtime from inside async context is eliminated entirely.

use crate::error::{AppError, AppResult};
use serde::Deserialize;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Deserialize)]
pub struct SeedRow {
    pub id: String,
    pub label: String,
    pub default: bool,
    pub seed_hex: String,
}

/// Bouncer HTTP client.
#[derive(Clone)]
pub struct BouncerClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl BouncerClient {
    /// Build a new client. Constructs the inner blocking HTTP client eagerly
    /// so its tokio runtime is allocated on a dedicated background thread
    /// before any async task that wraps a blocking call ever runs. Callers
    /// SHOULD invoke this from a sync startup context.
    pub fn new(base: impl Into<String>) -> Self {
        let http = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .expect("build blocking http client");
        Self {
            base: base.into(),
            http,
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    // ---- blocking variants (startup / CLI / sync tests only) -----------

    /// Poll `GET /health` with exponential backoff (200ms → 2s cap) until
    /// the bouncer answers 200 OK or the timeout elapses. Used at startup
    /// after spawning the sidecar to make sure it has bound its port
    /// before we try to fetch seeds.
    pub fn wait_for_ready_blocking(&self, timeout: Duration) -> AppResult<()> {
        let start = Instant::now();
        let mut delay = Duration::from_millis(200);
        loop {
            match self.health_blocking() {
                Ok(()) => return Ok(()),
                Err(e) => {
                    if start.elapsed() >= timeout {
                        return Err(AppError::Internal(format!(
                            "bouncer not ready after {}s: {e}",
                            timeout.as_secs()
                        )));
                    }
                    std::thread::sleep(delay);
                    delay = (delay * 2).min(Duration::from_secs(2));
                }
            }
        }
    }

    /// Async wrapper around [`Self::wait_for_ready_blocking`]. Safe to call
    /// from a tokio context (the blocking poll runs on the blocking pool).
    pub async fn wait_for_ready(&self, timeout: Duration) -> AppResult<()> {
        let me = self.clone();
        spawn_blocking_result(move || me.wait_for_ready_blocking(timeout)).await
    }

    pub fn health_blocking(&self) -> AppResult<()> {
        let r = self
            .http
            .get(format!("{}/health", self.base))
            .send()
            .map_err(|e| AppError::Internal(format!("bouncer health: {e}")))?;
        if !r.status().is_success() {
            return Err(AppError::Internal(format!("bouncer health {}", r.status())));
        }
        Ok(())
    }

    pub fn list_seeds_blocking(&self) -> AppResult<Vec<SeedRow>> {
        let r = self
            .http
            .get(format!("{}/seeds", self.base))
            .send()
            .map_err(|e| AppError::Internal(format!("bouncer seeds: {e}")))?;
        if !r.status().is_success() {
            return Err(AppError::Internal(format!("bouncer seeds {}", r.status())));
        }
        r.json()
            .map_err(|e| AppError::Internal(format!("bouncer seeds parse: {e}")))
    }

    pub fn print_blocking(
        &self,
        kind: &str,
        payload: &serde_json::Value,
        target: Option<&str>,
    ) -> AppResult<()> {
        let body = serde_json::json!({
            "kind": kind,
            "payload": payload,
            "target_printer_id": target,
        });
        let r = self
            .http
            .post(format!("{}/print", self.base))
            .json(&body)
            .send()
            .map_err(|e| AppError::Internal(format!("bouncer print: {e}")))?;
        if !r.status().is_success() {
            return Err(AppError::Internal(format!("bouncer print {}", r.status())));
        }
        Ok(())
    }

    pub fn post_report_blocking(
        &self,
        business_day: &str,
        generated_at: i64,
        report: &serde_json::Value,
    ) -> AppResult<()> {
        let body = serde_json::json!({
            "business_day": business_day,
            "generated_at": generated_at,
            "report": report,
        });
        let r = self
            .http
            .post(format!("{}/reports/eod", self.base))
            .json(&body)
            .send()
            .map_err(|e| AppError::Internal(format!("bouncer report: {e}")))?;
        if !r.status().is_success() {
            return Err(AppError::Internal(format!("bouncer report {}", r.status())));
        }
        Ok(())
    }

    // ---- async wrappers (any tokio task) -------------------------------

    pub async fn print(
        &self,
        kind: &str,
        payload: &serde_json::Value,
        target: Option<&str>,
    ) -> AppResult<()> {
        let me = self.clone();
        let kind = kind.to_string();
        let payload = payload.clone();
        let target = target.map(|s| s.to_string());
        spawn_blocking_result(move || me.print_blocking(&kind, &payload, target.as_deref())).await
    }

    pub async fn post_report(
        &self,
        business_day: &str,
        generated_at: i64,
        report: &serde_json::Value,
    ) -> AppResult<()> {
        let me = self.clone();
        let day = business_day.to_string();
        let report = report.clone();
        spawn_blocking_result(move || me.post_report_blocking(&day, generated_at, &report)).await
    }
}

/// Helper: run a blocking HTTP closure on the blocking pool and join.
async fn spawn_blocking_result<T, F>(f: F) -> AppResult<T>
where
    F: FnOnce() -> AppResult<T> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .unwrap_or_else(|e| Err(AppError::Internal(format!("bouncer join: {e}"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::{get, post};
    use std::net::SocketAddr;

    /// Spawn a hand-rolled axum stub on a random port and return (base_url, shutdown_tx).
    async fn spawn_stub() -> (String, tokio::sync::oneshot::Sender<()>) {
        let app = axum::Router::new()
            .route("/health", get(|| async { Json(serde_json::json!({"ok": true})) }))
            .route(
                "/seeds",
                get(|| async {
                    Json(serde_json::json!([{
                        "id": "stub-1",
                        "label": "Stub seed",
                        "default": true,
                        "seed_hex": "0000000000000000000000000000000000000000000000000000000000000001"
                    }]))
                }),
            )
            .route(
                "/print",
                post(|axum::Json(_v): axum::Json<serde_json::Value>| async {
                    Json(serde_json::json!({"queued": true}))
                }),
            )
            .route(
                "/reports/eod",
                post(|axum::Json(_v): axum::Json<serde_json::Value>| async {
                    Json(serde_json::json!({"stored": true}))
                }),
            );

        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });
        (format!("http://{addr}"), tx)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn health_blocking_ok_via_spawn_blocking() {
        let (base, shutdown) = spawn_stub().await;
        tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client.health_blocking().unwrap();
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_seeds_blocking_returns_one() {
        let (base, shutdown) = spawn_stub().await;
        let rows = tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client.list_seeds_blocking().unwrap()
        })
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "stub-1");
        assert!(rows[0].default);
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn print_async_ok() {
        let (base, shutdown) = spawn_stub().await;
        // Build the client on the blocking pool so its inner reqwest
        // runtime is owned by a thread we're allowed to drop later (the
        // production startup path runs `BouncerClient::new` from a
        // genuinely sync context, so this isn't a real-world concern).
        let client = tokio::task::spawn_blocking(move || BouncerClient::new(base))
            .await
            .unwrap();
        client
            .print("kitchen", &serde_json::json!({"x":1}), Some("kitchen-1"))
            .await
            .unwrap();
        // Drop the client off-runtime too.
        tokio::task::spawn_blocking(move || drop(client)).await.unwrap();
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn post_report_async_ok() {
        let (base, shutdown) = spawn_stub().await;
        let client = tokio::task::spawn_blocking(move || BouncerClient::new(base))
            .await
            .unwrap();
        client
            .post_report("2026-04-29", 1_700_000_000, &serde_json::json!({"k":"v"}))
            .await
            .unwrap();
        tokio::task::spawn_blocking(move || drop(client)).await.unwrap();
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_for_ready_succeeds_once_endpoint_appears() {
        // Bind the listener immediately, but only start serving /health
        // after a short delay — wait_for_ready should keep polling and
        // succeed within the timeout.
        let listener = tokio::net::TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .unwrap();
        let addr = listener.local_addr().unwrap();
        let base = format!("http://{addr}");
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();

        tokio::spawn(async move {
            // Delay before accepting/serving so the first few /health
            // probes from wait_for_ready fail with a connection error.
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            let app = axum::Router::new().route(
                "/health",
                get(|| async { Json(serde_json::json!({"ok": true})) }),
            );
            let _ = axum::serve(listener, app)
                .with_graceful_shutdown(async move {
                    let _ = rx.await;
                })
                .await;
        });

        let result = tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client.wait_for_ready_blocking(std::time::Duration::from_secs(5))
        })
        .await
        .unwrap();
        assert!(result.is_ok(), "wait_for_ready should succeed: {result:?}");
        let _ = tx.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wait_for_ready_times_out_when_unreachable() {
        let result = tokio::task::spawn_blocking(|| {
            // Port 1 is reserved/closed; connect attempts fail fast.
            let client = BouncerClient::new("http://127.0.0.1:1");
            client.wait_for_ready_blocking(std::time::Duration::from_millis(500))
        })
        .await
        .unwrap();
        let err = result.expect_err("wait_for_ready must time out");
        assert!(
            err.to_string().contains("bouncer not ready"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn health_blocking_fails_when_unreachable() {
        let r = tokio::task::spawn_blocking(|| {
            let client = BouncerClient::new("http://127.0.0.1:1");
            client.health_blocking()
        })
        .await
        .unwrap();
        assert!(r.is_err());
    }
}
