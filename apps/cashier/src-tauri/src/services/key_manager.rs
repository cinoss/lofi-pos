//! Business-day-keyed Data Encryption Key derivation.
//!
//! After the bouncer integration there are no on-disk DEK rows. Instead
//! every DEK is derived deterministically from a seed (held in the
//! [`SeedCache`]) and the event's business_day:
//!
//! ```text
//! dek = blake3_keyed(seed, business_day)
//! ```
//!
//! The cashier ships with a hard-coded fallback seed; if the bouncer is
//! reachable, real seeds replace the default. Tagged onto each event row
//! is the `seed_id` used at write time, so reads pick the right seed.

use crate::bouncer::seed_cache::SeedCache;
use crate::crypto::Dek;
use crate::error::AppResult;
use std::sync::Arc;

pub struct KeyManager {
    cache: Arc<SeedCache>,
}

impl KeyManager {
    pub fn new(cache: Arc<SeedCache>) -> Self {
        Self { cache }
    }

    /// DEK + seed_id for encrypting an event written today.
    pub fn current_dek(&self, business_day: &str) -> (Dek, String) {
        let seed = self.cache.default_seed();
        (
            derive_dek(seed, business_day),
            self.cache.default_id().to_string(),
        )
    }

    /// DEK for decrypting an event tagged with `seed_id` and `business_day`.
    /// Returns `Crypto("seed expired …")` if the seed is no longer in the cache.
    pub fn dek_for(&self, seed_id: &str, business_day: &str) -> AppResult<Dek> {
        let seed = self.cache.get(seed_id)?;
        Ok(derive_dek(seed, business_day))
    }
}

fn derive_dek(seed: &[u8; 32], business_day: &str) -> Dek {
    let mut hasher = blake3::Hasher::new_keyed(seed);
    hasher.update(business_day.as_bytes());
    let out = hasher.finalize();
    Dek::from_bytes(out.as_bytes()).expect("blake3 output is 32 bytes")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    fn cache() -> Arc<SeedCache> {
        Arc::new(SeedCache::from_seeds(
            "primary",
            vec![
                ("primary".into(), [1u8; 32]),
                ("secondary".into(), [2u8; 32]),
            ],
        ))
    }

    #[test]
    fn current_dek_roundtrips_with_dek_for_for_same_inputs() {
        let km = KeyManager::new(cache());
        let (dek, seed_id) = km.current_dek("2026-04-28");
        let blob = dek.encrypt(b"hi", b"aad").unwrap();
        let dek2 = km.dek_for(&seed_id, "2026-04-28").unwrap();
        assert_eq!(dek2.decrypt(&blob, b"aad").unwrap(), b"hi");
    }

    #[test]
    fn dek_for_unknown_seed_returns_crypto_error() {
        let km = KeyManager::new(cache());
        let err = km
            .dek_for("never-existed", "2026-04-28")
            .err()
            .expect("expected error");
        match err {
            AppError::Crypto(msg) => assert!(msg.contains("seed expired")),
            other => panic!("expected Crypto, got {other:?}"),
        }
    }

    #[test]
    fn different_days_yield_different_deks() {
        let km = KeyManager::new(cache());
        let (a, _) = km.current_dek("2026-04-28");
        let (b, _) = km.current_dek("2026-04-29");
        let blob = a.encrypt(b"x", b"a").unwrap();
        assert!(b.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn different_seeds_yield_different_deks() {
        let km = KeyManager::new(cache());
        let a = km.dek_for("primary", "2026-04-28").unwrap();
        let b = km.dek_for("secondary", "2026-04-28").unwrap();
        let blob = a.encrypt(b"x", b"a").unwrap();
        assert!(b.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn current_dek_uses_cache_default_id() {
        let km = KeyManager::new(cache());
        let (_, sid) = km.current_dek("2026-04-28");
        assert_eq!(sid, "primary");
    }
}
