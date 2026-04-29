//! In-RAM seed cache backed by the bouncer.
//!
//! The cashier never writes seeds to disk. At startup we fetch the active
//! seed list from the bouncer and hold the bytes in memory. If the bouncer
//! is unreachable we degrade to a hard-coded fallback seed so the cashier
//! can still run (and so that any events written across degraded restarts
//! remain decryptable).

use crate::bouncer::client::BouncerClient;
use crate::error::{AppError, AppResult};
use std::collections::HashMap;

/// Deterministic fallback seed id. Tagged on every event written while the
/// cashier is in degraded mode.
pub const FALLBACK_SEED_ID: &str = "fallback";

/// Hard-coded fallback seed material. Derived from a fixed BLAKE3 hash so it
/// is identical across builds — events written in degraded mode on one boot
/// remain decryptable on any subsequent boot, regardless of whether the
/// bouncer is reachable.
pub fn fallback_seed_bytes() -> [u8; 32] {
    *blake3::hash(b"lofi-pos-fallback-seed-2026-v1").as_bytes()
}

pub struct SeedCache {
    by_id: HashMap<String, [u8; 32]>,
    default_id: String,
    pub degraded: bool,
}

impl SeedCache {
    /// Try to populate the cache from the bouncer; on any failure (network,
    /// no seeds, no default) fall back to fallback-only and mark the cache
    /// as `degraded`. Always returns Ok so cashier startup is never blocked.
    pub fn fetch_or_fallback(client: &BouncerClient) -> Self {
        let mut by_id: HashMap<String, [u8; 32]> = HashMap::new();
        // Always include the fallback so degraded-mode events written across
        // boots remain decryptable.
        by_id.insert(FALLBACK_SEED_ID.to_string(), fallback_seed_bytes());

        match client.list_seeds() {
            Ok(rows) if !rows.is_empty() => {
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
                    let mut arr = [0u8; 32];
                    arr.copy_from_slice(&bytes);
                    if r.default {
                        bouncer_default = Some(r.id.clone());
                    }
                    by_id.insert(r.id, arr);
                }
                if let Some(default_id) = bouncer_default {
                    return Self {
                        by_id,
                        default_id,
                        degraded: false,
                    };
                }
                tracing::warn!("bouncer returned no default seed; entering degraded mode");
            }
            Ok(_) => tracing::warn!("bouncer returned zero seeds; entering degraded mode"),
            Err(e) => {
                tracing::warn!(error = %e, "bouncer unreachable; entering degraded mode")
            }
        }
        Self {
            by_id,
            default_id: FALLBACK_SEED_ID.to_string(),
            degraded: true,
        }
    }

    /// Build a fully-populated, non-degraded cache from explicit (id, bytes)
    /// pairs. Used by tests and `eod::test_support`; not called from the
    /// production startup path (which goes through `fetch_or_fallback`).
    pub fn from_seeds(default_id: impl Into<String>, seeds: Vec<(String, [u8; 32])>) -> Self {
        let mut by_id = HashMap::new();
        by_id.insert(FALLBACK_SEED_ID.to_string(), fallback_seed_bytes());
        for (id, b) in seeds {
            by_id.insert(id, b);
        }
        let default_id = default_id.into();
        assert!(by_id.contains_key(&default_id), "default not in seeds");
        Self {
            by_id,
            default_id,
            degraded: false,
        }
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
            .ok_or_else(|| AppError::Crypto(format!("seed expired for id {id}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fetch_with_unreachable_bouncer_yields_degraded_with_fallback() {
        let client = BouncerClient::new("http://127.0.0.1:1");
        let cache = SeedCache::fetch_or_fallback(&client);
        assert!(cache.degraded);
        assert_eq!(cache.default_id(), FALLBACK_SEED_ID);
        assert!(cache.get(FALLBACK_SEED_ID).is_ok());
    }

    #[test]
    fn fallback_bytes_are_stable_across_calls() {
        let c1 = SeedCache::fetch_or_fallback(&BouncerClient::new("http://127.0.0.1:1"));
        let c2 = SeedCache::fetch_or_fallback(&BouncerClient::new("http://127.0.0.1:1"));
        assert_eq!(c1.default_seed(), c2.default_seed());
    }

    #[test]
    fn unknown_seed_id_returns_crypto_error() {
        let cache = SeedCache::fetch_or_fallback(&BouncerClient::new("http://127.0.0.1:1"));
        let err = cache.get("not-a-real-seed").unwrap_err();
        match err {
            AppError::Crypto(msg) => assert!(msg.contains("seed expired")),
            other => panic!("expected Crypto, got {other:?}"),
        }
    }

    #[test]
    fn from_seeds_test_helper_builds_non_degraded() {
        let cache = SeedCache::from_seeds("a", vec![("a".into(), [9u8; 32])]);
        assert!(!cache.degraded);
        assert_eq!(cache.default_id(), "a");
        assert_eq!(cache.default_seed(), &[9u8; 32]);
    }
}
