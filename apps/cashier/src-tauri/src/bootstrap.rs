use crate::error::AppResult;
use crate::keychain::KeyStore;

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
