use crate::error::{AppError, AppResult};
use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use rand::RngCore;
use zeroize::ZeroizeOnDrop;

/// AES-256 key length in bytes.
pub const KEY_LEN: usize = 32;
/// AES-GCM nonce length in bytes (96 bits, the spec-recommended size).
pub const NONCE_LEN: usize = 12;
/// AES-GCM authentication tag length in bytes.
pub const TAG_LEN: usize = 16;

/// Data Encryption Key. Derived deterministically per business day from a
/// seed held in the bouncer-backed `SeedCache`. Never persisted in the
/// cashier — recomputed on demand from `(seed, business_day)`.
#[derive(ZeroizeOnDrop)]
pub struct Dek([u8; KEY_LEN]);

impl Dek {
    /// Generate a fresh random DEK from the OS CSPRNG. Used by tests; the
    /// production path always derives via `KeyManager`.
    pub fn new_random() -> Self {
        let mut k = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut k);
        Self(k)
    }

    /// Reconstruct a DEK from raw bytes (e.g. blake3 output). Rejects any
    /// length other than `KEY_LEN`.
    pub fn from_bytes(b: &[u8]) -> AppResult<Self> {
        if b.len() != KEY_LEN {
            return Err(AppError::Crypto("bad dek length".into()));
        }
        let mut k = [0u8; KEY_LEN];
        k.copy_from_slice(b);
        Ok(Self(k))
    }

    /// Borrow the raw key material (only used by tests).
    #[cfg(test)]
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }

    /// Encrypt a payload with this DEK. Output layout: `nonce || ct || tag`.
    /// The AAD binds the ciphertext to its context (e.g. event id) and must
    /// be supplied identically on decrypt.
    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
        encrypt(&self.0, plaintext, aad)
    }
    /// Decrypt a blob produced by `encrypt`. Returns an error if the AAD
    /// differs or the ciphertext/tag has been tampered with.
    pub fn decrypt(&self, blob: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
        decrypt(&self.0, blob, aad)
    }
}

fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .map_err(|e| AppError::Crypto(format!("encrypt: {e}")))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn decrypt(key: &[u8; KEY_LEN], blob: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    if blob.len() < NONCE_LEN + TAG_LEN {
        return Err(AppError::Crypto("blob too short".into()));
    }
    let cipher = Aes256Gcm::new(key.into());
    let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, Payload { msg: ct, aad })
        .map_err(|e| AppError::Crypto(format!("decrypt: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dek_roundtrip() {
        let dek = Dek::new_random();
        let pt = b"hello world";
        let blob = dek.encrypt(pt, b"aad-1").unwrap();
        assert_eq!(dek.decrypt(&blob, b"aad-1").unwrap(), pt);
    }

    #[test]
    fn dek_decrypt_fails_on_wrong_aad() {
        let dek = Dek::new_random();
        let blob = dek.encrypt(b"x", b"a").unwrap();
        assert!(dek.decrypt(&blob, b"b").is_err());
    }

    #[test]
    fn dek_decrypt_fails_on_tamper() {
        let dek = Dek::new_random();
        let mut blob = dek.encrypt(b"x", b"a").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(dek.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn dek_decrypt_fails_with_wrong_key() {
        let d1 = Dek::new_random();
        let d2 = Dek::new_random();
        let blob = d1.encrypt(b"x", b"a").unwrap();
        assert!(d2.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn dek_from_bytes_rejects_wrong_length() {
        assert!(Dek::from_bytes(&[0u8; 31]).is_err());
        assert!(Dek::from_bytes(&[0u8; 33]).is_err());
        assert!(Dek::from_bytes(&[0u8; 32]).is_ok());
    }

    #[test]
    fn nonce_uniqueness_smoke() {
        let dek = Dek::new_random();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let blob = dek.encrypt(b"x", b"a").unwrap();
            let nonce = blob[..NONCE_LEN].to_vec();
            assert!(seen.insert(nonce), "nonce collision");
        }
    }

    use proptest::prelude::*;
    proptest! {
        #[test]
        fn prop_dek_roundtrip(pt in proptest::collection::vec(any::<u8>(), 0..4096),
                              aad in proptest::collection::vec(any::<u8>(), 0..256)) {
            let dek = Dek::new_random();
            let blob = dek.encrypt(&pt, &aad).unwrap();
            prop_assert_eq!(dek.decrypt(&blob, &aad).unwrap(), pt);
        }
    }
}
