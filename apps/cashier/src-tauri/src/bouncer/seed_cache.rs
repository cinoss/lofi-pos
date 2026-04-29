//! In-RAM seed cache backed by the bouncer.
//!
//! The cashier never writes seeds to disk. At startup we fetch the active
//! seed list from the bouncer and hold the bytes in memory. The cashier is
//! ignorant of any fallback semantics — the bouncer (built by another team)
//! handles its own internal fallback and is expected to always return at
//! least one seed (with one marked `default`) when reachable. If the bouncer
//! is unreachable, returns an empty list, or returns no default, the cashier
//! hard-fails at startup.

use crate::bouncer::client::BouncerClient;
use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use zeroize::Zeroizing;

/// Wrap raw seed material so it is wiped from memory on drop.
type SeedBytes = Zeroizing<[u8; 32]>;

pub struct SeedCache {
    by_id: HashMap<String, SeedBytes>,
    default_id: String,
}

impl std::fmt::Debug for SeedCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SeedCache")
            .field("default_id", &self.default_id)
            .field("seed_count", &self.by_id.len())
            .finish()
    }
}

impl SeedCache {
    /// Populate the cache from the bouncer. Hard-fails if the bouncer is
    /// unreachable, returns no seeds, or returns no default seed.
    pub fn fetch(client: &BouncerClient) -> AppResult<Self> {
        let rows = client
            .list_seeds_blocking()
            .map_err(|e| AppError::Internal(format!("bouncer unreachable: {e}")))?;
        if rows.is_empty() {
            return Err(AppError::Internal(
                "bouncer returned zero seeds".to_string(),
            ));
        }
        let mut by_id: HashMap<String, SeedBytes> = HashMap::new();
        let mut bouncer_default: Option<String> = None;
        for r in rows {
            let bytes = match hex::decode(&r.seed_hex) {
                Ok(b) if b.len() == 32 => b,
                _ => {
                    tracing::error!(
                        seed_id = %r.id,
                        "bouncer returned malformed seed; ignoring"
                    );
                    continue;
                }
            };
            let mut arr = Zeroizing::new([0u8; 32]);
            arr.copy_from_slice(&bytes);
            if r.default {
                bouncer_default = Some(r.id.clone());
            }
            by_id.insert(r.id, arr);
        }
        let default_id = bouncer_default.ok_or_else(|| {
            AppError::Internal("bouncer returned no default seed".to_string())
        })?;
        Ok(Self { by_id, default_id })
    }

    /// Build a fully-populated cache from explicit (id, bytes) pairs. Used by
    /// tests and `eod::test_support`; not called from the production startup
    /// path (which goes through `fetch`).
    pub fn from_seeds(default_id: impl Into<String>, seeds: Vec<(String, [u8; 32])>) -> Self {
        let mut by_id: HashMap<String, SeedBytes> = HashMap::new();
        for (id, b) in seeds {
            by_id.insert(id, Zeroizing::new(b));
        }
        let default_id = default_id.into();
        assert!(by_id.contains_key(&default_id), "default not in seeds");
        Self { by_id, default_id }
    }

    pub fn default_id(&self) -> &str {
        &self.default_id
    }

    pub fn default_seed(&self) -> &[u8; 32] {
        // Safe: constructors guarantee default_id is in by_id.
        self.by_id.get(&self.default_id).expect("default seed present")
    }

    pub fn get(&self, id: &str) -> AppResult<&[u8; 32]> {
        self.by_id
            .get(id)
            .map(|z| &**z)
            .ok_or_else(|| AppError::Crypto(format!("seed expired for id {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Json;
    use axum::routing::get;
    use std::net::SocketAddr;

    #[test]
    fn fetch_with_unreachable_bouncer_returns_err() {
        let client = BouncerClient::new("http://127.0.0.1:1");
        let err = SeedCache::fetch(&client).unwrap_err();
        match err {
            AppError::Internal(msg) => assert!(msg.contains("bouncer unreachable")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[test]
    fn unknown_seed_id_returns_crypto_error() {
        let cache = SeedCache::from_seeds("a", vec![("a".into(), [9u8; 32])]);
        let err = cache.get("not-a-real-seed").unwrap_err();
        match err {
            AppError::Crypto(msg) => assert!(msg.contains("seed expired")),
            other => panic!("expected Crypto, got {other:?}"),
        }
    }

    #[test]
    fn from_seeds_test_helper_builds_cache() {
        let cache = SeedCache::from_seeds("a", vec![("a".into(), [9u8; 32])]);
        assert_eq!(cache.default_id(), "a");
        assert_eq!(cache.default_seed(), &[9u8; 32]);
    }

    /// Spawn an axum stub returning the supplied JSON body for `/seeds`.
    async fn spawn_seeds_stub(body: serde_json::Value) -> (String, tokio::sync::oneshot::Sender<()>) {
        let app = axum::Router::new().route(
            "/seeds",
            get(move || {
                let body = body.clone();
                async move { Json(body) }
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
    async fn fetch_with_empty_seed_list_returns_err() {
        let body = serde_json::json!([]);
        let (base, shutdown) = spawn_seeds_stub(body).await;
        let err = tokio::task::spawn_blocking(move || {
            SeedCache::fetch(&BouncerClient::new(base)).unwrap_err()
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
        match err {
            AppError::Internal(msg) => assert!(msg.contains("zero seeds")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fetch_with_no_default_returns_err() {
        let body = serde_json::json!([
            {
                "id": "a",
                "label": "A",
                "default": false,
                "seed_hex": "0000000000000000000000000000000000000000000000000000000000000001",
            }
        ]);
        let (base, shutdown) = spawn_seeds_stub(body).await;
        let err = tokio::task::spawn_blocking(move || {
            SeedCache::fetch(&BouncerClient::new(base)).unwrap_err()
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
        match err {
            AppError::Internal(msg) => assert!(msg.contains("no default seed")),
            other => panic!("expected Internal, got {other:?}"),
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn fetch_with_healthy_bouncer_returns_ok() {
        let body = serde_json::json!([
            {
                "id": "primary",
                "label": "Primary",
                "default": true,
                "seed_hex": "0000000000000000000000000000000000000000000000000000000000000001",
            },
            {
                "id": "secondary",
                "label": "Secondary",
                "default": false,
                "seed_hex": "0000000000000000000000000000000000000000000000000000000000000002",
            }
        ]);
        let (base, shutdown) = spawn_seeds_stub(body).await;
        let cache = tokio::task::spawn_blocking(move || {
            SeedCache::fetch(&BouncerClient::new(base)).unwrap()
        })
        .await
        .unwrap();
        let _ = shutdown.send(());
        assert_eq!(cache.default_id(), "primary");
        assert!(cache.get("primary").is_ok());
        assert!(cache.get("secondary").is_ok());
    }
}
