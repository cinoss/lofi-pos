use crate::error::{AppError, AppResult};
use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Argon2,
};

/// Minimum PIN length enforced by `hash_pin`. `verify_pin` does NOT check
/// length — it accepts any input PIN since stored hashes might predate this
/// schema. Only fresh PINs (admin creates staff or rotates PIN) are gated.
pub const MIN_PIN_LENGTH: usize = 6;

/// Hash a PIN with Argon2id. Output is the encoded `$argon2id$...` string,
/// suitable for storage in `staff.pin_hash`.
pub fn hash_pin(pin: &str) -> AppResult<String> {
    if pin.chars().count() < MIN_PIN_LENGTH {
        return Err(AppError::Validation(format!(
            "pin must be at least {MIN_PIN_LENGTH} characters"
        )));
    }
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    Ok(argon
        .hash_password(pin.as_bytes(), &salt)
        .map_err(|e| AppError::Crypto(format!("hash_pin: {e}")))?
        .to_string())
}

/// Verify a PIN against a stored Argon2 hash. Returns Ok(true) on match,
/// Ok(false) on mismatch, Err on malformed hash.
pub fn verify_pin(pin: &str, stored_hash: &str) -> AppResult<bool> {
    let parsed = PasswordHash::new(stored_hash)
        .map_err(|e| AppError::Crypto(format!("verify_pin parse: {e}")))?;
    Ok(Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrip() {
        let h = hash_pin("123456").unwrap();
        assert!(verify_pin("123456", &h).unwrap());
    }

    #[test]
    fn wrong_pin_does_not_verify() {
        let h = hash_pin("123456").unwrap();
        assert!(!verify_pin("000000", &h).unwrap());
    }

    #[test]
    fn malformed_hash_errors() {
        assert!(verify_pin("123456", "not-a-hash").is_err());
    }

    #[test]
    fn two_hashes_of_same_pin_differ() {
        // Argon2 uses random salt; same plaintext → different ciphertext
        let a = hash_pin("123456").unwrap();
        let b = hash_pin("123456").unwrap();
        assert_ne!(a, b);
        assert!(verify_pin("123456", &a).unwrap());
        assert!(verify_pin("123456", &b).unwrap());
    }

    #[test]
    fn hash_pin_rejects_short() {
        let r = hash_pin("12345");
        assert!(matches!(r, Err(AppError::Validation(_))));
        // Boundary: exactly MIN_PIN_LENGTH passes.
        assert!(hash_pin("123456").is_ok());
        // Empty input also rejected.
        assert!(matches!(hash_pin(""), Err(AppError::Validation(_))));
    }
}
