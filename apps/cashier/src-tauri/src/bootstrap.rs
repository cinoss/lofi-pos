use crate::crypto::Kek;
use crate::error::AppResult;
use crate::keychain::KeyStore;

/// Keystore entry name under which the KEK is persisted.
pub(crate) const KEK_NAME: &str = "kek";

pub(crate) const AUTH_SIGNING_NAME: &str = "auth-signing";
pub(crate) const AUTH_SIGNING_LEN: usize = 32;

/// Load existing auth signing key from keystore or generate + persist a fresh one.
/// Used by `auth::token::sign`/`verify`.
pub fn load_or_init_auth_signing(ks: &dyn KeyStore) -> AppResult<Vec<u8>> {
    if let Some(bytes) = ks.get(AUTH_SIGNING_NAME)? {
        if bytes.len() == AUTH_SIGNING_LEN {
            tracing::info!("auth signing key loaded from keystore");
            return Ok(bytes);
        }
        return Err(crate::error::AppError::Crypto(
            "stored auth signing key has wrong length".into(),
        ));
    }
    use rand::RngCore;
    let mut bytes = vec![0u8; AUTH_SIGNING_LEN];
    rand::thread_rng().fill_bytes(&mut bytes);
    ks.set(AUTH_SIGNING_NAME, &bytes)?;
    tracing::info!("auth signing key generated and stored (first run)");
    Ok(bytes)
}

/// Load the KEK from the given keystore, generating and persisting one on
/// first run. Errors do NOT regenerate: a parse failure on existing material
/// is propagated rather than silently destroying every blob encrypted under
/// the previous KEK.
pub fn load_or_init_kek(ks: &dyn KeyStore) -> AppResult<Kek> {
    if let Some(bytes) = ks.get(KEK_NAME)? {
        let kek = Kek::from_bytes(&bytes)?;
        tracing::info!("kek loaded from keystore");
        return Ok(kek);
    }
    let kek = Kek::new_random();
    ks.set(KEK_NAME, kek.as_bytes())?;
    tracing::info!("kek generated and stored (first run)");
    Ok(kek)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::test_support::MemKeyStore;

    #[test]
    fn first_run_generates_and_stores_kek() {
        let ks = MemKeyStore::default();
        let kek = load_or_init_kek(&ks).unwrap();
        let stored = ks.get(KEK_NAME).unwrap().expect("kek should be stored");
        assert_eq!(stored.as_slice(), kek.as_bytes());
    }

    #[test]
    fn second_run_returns_same_kek() {
        let ks = MemKeyStore::default();
        let k1 = load_or_init_kek(&ks).unwrap();
        let k2 = load_or_init_kek(&ks).unwrap();
        assert_eq!(k1.as_bytes(), k2.as_bytes());
    }

    #[test]
    fn corrupt_stored_kek_returns_error() {
        let ks = MemKeyStore::default();
        ks.set(KEK_NAME, &[0u8; 16]).unwrap();
        assert!(load_or_init_kek(&ks).is_err());
    }
}

#[cfg(test)]
mod auth_signing_tests {
    use super::*;
    use crate::keychain::test_support::MemKeyStore;

    #[test]
    fn first_run_generates_auth_signing() {
        let ks = MemKeyStore::default();
        let k = load_or_init_auth_signing(&ks).unwrap();
        assert_eq!(k.len(), AUTH_SIGNING_LEN);
        assert_eq!(ks.get(AUTH_SIGNING_NAME).unwrap().as_deref(), Some(&k[..]));
    }

    #[test]
    fn second_run_returns_same_auth_signing() {
        let ks = MemKeyStore::default();
        let a = load_or_init_auth_signing(&ks).unwrap();
        let b = load_or_init_auth_signing(&ks).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn wrong_length_auth_signing_returns_error() {
        let ks = MemKeyStore::default();
        ks.set(AUTH_SIGNING_NAME, &[0u8; 16]).unwrap();
        assert!(load_or_init_auth_signing(&ks).is_err());
    }
}
