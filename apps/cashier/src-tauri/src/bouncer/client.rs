//! HTTP client for the bouncer sidecar (localhost:7879 by default).
//!
//! Wire protocol per `docs/superpowers/specs/2026-04-29-bouncer-sidecar-design.md`.
//! Blocking reqwest is used to keep call sites simple; the cashier server task
//! calls these from `spawn_blocking` or background workers.

use crate::error::{AppError, AppResult};
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SeedRow {
    pub id: String,
    pub label: String,
    pub default: bool,
    pub seed_hex: String,
}

pub struct BouncerClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl BouncerClient {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .expect("build blocking http client"),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn health(&self) -> AppResult<()> {
        let r = self
            .http
            .get(format!("{}/health", self.base))
            .send()
            .map_err(|e| AppError::Internal(format!("bouncer health: {e}")))?;
        if !r.status().is_success() {
            return Err(AppError::Internal(format!(
                "bouncer health {}",
                r.status()
            )));
        }
        Ok(())
    }

    pub fn list_seeds(&self) -> AppResult<Vec<SeedRow>> {
        let r = self
            .http
            .get(format!("{}/seeds", self.base))
            .send()
            .map_err(|e| AppError::Internal(format!("bouncer seeds: {e}")))?;
        if !r.status().is_success() {
            return Err(AppError::Internal(format!(
                "bouncer seeds {}",
                r.status()
            )));
        }
        r.json()
            .map_err(|e| AppError::Internal(format!("bouncer seeds parse: {e}")))
    }

    pub fn print(
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
            return Err(AppError::Internal(format!(
                "bouncer print {}",
                r.status()
            )));
        }
        Ok(())
    }

    pub fn post_report(
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
            return Err(AppError::Internal(format!(
                "bouncer report {}",
                r.status()
            )));
        }
        Ok(())
    }
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
    async fn health_ok() {
        let (base, shutdown) = spawn_stub().await;
        tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client.health().unwrap();
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn list_seeds_returns_one() {
        let (base, shutdown) = spawn_stub().await;
        let rows = tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client.list_seeds().unwrap()
        })
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].id, "stub-1");
        assert!(rows[0].default);
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn print_ok() {
        let (base, shutdown) = spawn_stub().await;
        tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client
                .print("kitchen", &serde_json::json!({"x":1}), Some("kitchen-1"))
                .unwrap();
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn post_report_ok() {
        let (base, shutdown) = spawn_stub().await;
        tokio::task::spawn_blocking(move || {
            let client = BouncerClient::new(base);
            client
                .post_report("2026-04-29", 1_700_000_000, &serde_json::json!({"k":"v"}))
                .unwrap();
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn health_fails_when_unreachable() {
        let r = tokio::task::spawn_blocking(|| {
            let client = BouncerClient::new("http://127.0.0.1:1");
            client.health()
        })
        .await
        .unwrap();
        assert!(r.is_err());
    }
}
