use crate::acl::role::Role;
use crate::error::{AppError, AppResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

fn default_role() -> Role {
    Role::Staff
}

/// JSON shape is part of the on-the-wire token contract. Adding fields
/// is backward-compatible if marked `#[serde(default)]`. Removing or
/// renaming fields breaks all existing tokens — bump the signing key
/// to invalidate old tokens during a migration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenClaims {
    pub staff_id: i64,
    #[serde(default = "default_role")]
    pub role: Role,
    /// unix-ms expiry
    pub exp: i64,
    /// unix-ms issued-at; populated by `AuthService.login` (Wave 3)
    #[serde(default)]
    pub iat: i64,
    /// Token id — UUID v4. Used for revocation/denylist lookup. Old tokens
    /// without `jti` deserialize to an empty string and are treated as
    /// "not revocable" (verify skips the denylist check). New tokens always
    /// carry a non-empty jti.
    #[serde(default)]
    pub jti: String,
}

/// Sign claims with the auth signing key. Output: base64url(claims_json).base64url(sig)
pub fn sign(claims: &TokenClaims, signing_key: &[u8]) -> AppResult<String> {
    let json = serde_json::to_vec(claims)
        .map_err(|e| AppError::Internal(format!("token serialize: {e}")))?;
    let body_b64 = URL_SAFE_NO_PAD.encode(&json);
    let mut mac = HmacSha256::new_from_slice(signing_key)
        .map_err(|e| AppError::Crypto(format!("hmac key: {e}")))?;
    mac.update(body_b64.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    Ok(format!("{body_b64}.{sig_b64}"))
}

/// Verify and parse a token. Checks HMAC (constant-time) and expiry.
pub fn verify(token: &str, signing_key: &[u8], now_ms: i64) -> AppResult<TokenClaims> {
    let Some((body_b64, sig_b64)) = token.split_once('.') else {
        tracing::debug!("token verify: malformed (no dot separator)");
        return Err(AppError::Unauthorized);
    };

    let mut mac = HmacSha256::new_from_slice(signing_key)
        .map_err(|e| AppError::Crypto(format!("hmac key: {e}")))?;
    mac.update(body_b64.as_bytes());
    let expected = mac.finalize().into_bytes();
    let provided = match URL_SAFE_NO_PAD.decode(sig_b64) {
        Ok(p) => p,
        Err(_) => {
            tracing::debug!("token verify: signature decode failed");
            return Err(AppError::Unauthorized);
        }
    };

    if !bool::from(provided.ct_eq(&expected[..])) {
        tracing::debug!("token verify: signature mismatch");
        return Err(AppError::Unauthorized);
    }

    let body = match URL_SAFE_NO_PAD.decode(body_b64) {
        Ok(b) => b,
        Err(_) => {
            tracing::debug!("token verify: body decode failed");
            return Err(AppError::Unauthorized);
        }
    };
    let claims: TokenClaims = match serde_json::from_slice(&body) {
        Ok(c) => c,
        Err(_) => {
            tracing::debug!("token verify: claims deserialize failed");
            return Err(AppError::Unauthorized);
        }
    };

    if now_ms >= claims.exp {
        tracing::debug!(now_ms, exp = claims.exp, "token verify: expired");
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        b"0123456789abcdef0123456789abcdef".to_vec()
    }
    fn claims(exp: i64) -> TokenClaims {
        TokenClaims {
            staff_id: 7,
            role: Role::Cashier,
            exp,
            iat: 0,
            jti: "test-jti".into(),
        }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let k = key();
        let t = sign(&claims(2_000_000_000_000), &k).unwrap();
        let parsed = verify(&t, &k, 1_000).unwrap();
        assert_eq!(parsed.staff_id, 7);
    }

    #[test]
    fn wrong_key_rejected() {
        let t = sign(&claims(2_000_000_000_000), &key()).unwrap();
        let other = b"ffffffffffffffffffffffffffffffff".to_vec();
        assert!(matches!(
            verify(&t, &other, 1_000),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn expired_token_rejected() {
        let k = key();
        let t = sign(&claims(100), &k).unwrap();
        assert!(matches!(verify(&t, &k, 200), Err(AppError::Unauthorized)));
    }

    #[test]
    fn tampered_body_rejected() {
        let k = key();
        let t_orig = sign(&claims(2_000_000_000_000), &k).unwrap();
        let mut bytes = t_orig.into_bytes();
        bytes[0] ^= 0x01;
        // May not be valid UTF-8 anymore, but verify takes &str via Strings — so wrap loosely:
        let t = String::from_utf8(bytes).unwrap_or_else(|e| {
            // Bytes are no longer UTF-8; for the purposes of verify they're still meant
            // to be parsed as a string. Fall back to lossy conversion (ASCII-tampered
            // base64 rarely produces non-UTF8 anyway).
            String::from_utf8_lossy(&e.into_bytes()).into_owned()
        });
        assert!(matches!(verify(&t, &k, 1_000), Err(AppError::Unauthorized)));
    }

    #[test]
    fn pre_wave2_token_defaults_role_to_staff() {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
        use hmac::{Hmac, Mac};
        use sha2::Sha256;

        let k = key();
        // Synthesize a pre-wave2-shape token: only staff_id + exp, no role/iat.
        let body = serde_json::json!({"staff_id": 1, "exp": 2_000_000_000_000_i64});
        let body_bytes = serde_json::to_vec(&body).unwrap();
        let body_b64 = URL_SAFE_NO_PAD.encode(&body_bytes);
        let mut mac = Hmac::<Sha256>::new_from_slice(&k).unwrap();
        mac.update(body_b64.as_bytes());
        let sig_b64 = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
        let t = format!("{body_b64}.{sig_b64}");

        let parsed = verify(&t, &k, 1_000).unwrap();
        assert_eq!(parsed.staff_id, 1);
        assert_eq!(parsed.role, crate::acl::role::Role::Staff);
        assert_eq!(parsed.iat, 0);
        assert_eq!(parsed.jti, "");
    }

    #[test]
    fn malformed_token_rejected() {
        assert!(matches!(
            verify("no-dot-here", &key(), 1_000),
            Err(AppError::Unauthorized)
        ));
    }
}
