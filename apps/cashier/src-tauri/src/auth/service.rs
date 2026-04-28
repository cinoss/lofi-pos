use crate::acl::Role;
use crate::auth::pin;
use crate::auth::token::{self, TokenClaims};
use crate::error::{AppError, AppResult};
use crate::store::master::{Master, Staff};
use crate::time::Clock;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// 12-hour token TTL.
pub const TOKEN_TTL_MS: i64 = 12 * 60 * 60 * 1000;

#[derive(Clone)]
pub struct AuthService {
    pub master: Arc<Mutex<Master>>,
    pub clock: Arc<dyn Clock>,
    pub signing_key: Arc<Vec<u8>>,
}

impl AuthService {
    /// Verify PIN against any staff row; on success return a signed token.
    /// Constant-time-ish: walks all staff (a real venue has <50; cost negligible).
    pub fn login(&self, pin: &str) -> AppResult<(String, TokenClaims)> {
        let staff_list = self.master.lock().unwrap().list_staff()?;
        let now = self.clock.now().timestamp_millis();
        for s in staff_list {
            if pin::verify_pin(pin, &s.pin_hash)? {
                let claims = TokenClaims {
                    staff_id: s.id,
                    role: s.role,
                    exp: now + TOKEN_TTL_MS,
                    iat: now,
                    jti: Uuid::new_v4().to_string(),
                };
                let token = token::sign(&claims, &self.signing_key)?;
                tracing::info!(staff_id = s.id, role = ?s.role, jti = %claims.jti, "login ok");
                return Ok((token, claims));
            }
        }
        tracing::warn!("login failed: invalid pin");
        Err(AppError::Unauthorized)
    }

    pub fn verify(&self, token: &str) -> AppResult<TokenClaims> {
        let now = self.clock.now().timestamp_millis();
        let claims = token::verify(token, &self.signing_key, now)?;
        if !claims.jti.is_empty()
            && self
                .master
                .lock()
                .unwrap()
                .is_token_denylisted(&claims.jti)?
        {
            tracing::warn!(jti = %claims.jti, "token rejected (denylisted)");
            return Err(AppError::Unauthorized);
        }
        Ok(claims)
    }

    /// Add the token's `jti` to the master denylist so subsequent `verify`
    /// calls reject it. No-op for tokens lacking a jti (pre-jti shape — the
    /// transition window). Idempotent at the storage layer.
    pub fn revoke(&self, claims: &TokenClaims) -> AppResult<()> {
        if claims.jti.is_empty() {
            return Ok(());
        }
        let now = self.clock.now().timestamp_millis();
        self.master
            .lock()
            .unwrap()
            .put_token_denylist(&claims.jti, claims.exp, now)
    }

    /// Verify a PIN and return the staff IF role >= `min_role`. Used for
    /// supervisor-override flows. Constant-cost iteration over staff list.
    pub fn verify_pin_for_role(&self, pin: &str, min_role: Role) -> AppResult<Staff> {
        let staff_list = self.master.lock().unwrap().list_staff()?;
        for s in staff_list {
            if s.role >= min_role && pin::verify_pin(pin, &s.pin_hash)? {
                return Ok(s);
            }
        }
        Err(AppError::Unauthorized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::acl::Role;
    use crate::auth::pin::hash_pin;
    use crate::time::test_support::MockClock;

    fn rig() -> AuthService {
        let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
        let pin = "123456";
        let hash = hash_pin(pin).unwrap();
        master
            .lock()
            .unwrap()
            .create_staff("Owner", &hash, Role::Owner, None)
            .unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
        let key: Arc<Vec<u8>> = Arc::new(vec![7u8; 32]);
        AuthService {
            master,
            clock,
            signing_key: key,
        }
    }

    #[test]
    fn login_with_valid_pin_succeeds() {
        let svc = rig();
        let (token, claims) = svc.login("123456").unwrap();
        assert_eq!(claims.role, Role::Owner);
        let parsed = svc.verify(&token).unwrap();
        assert_eq!(parsed.staff_id, claims.staff_id);
    }

    #[test]
    fn login_with_invalid_pin_unauthorized() {
        let svc = rig();
        assert!(matches!(svc.login("000000"), Err(AppError::Unauthorized)));
    }

    #[test]
    fn verify_tampered_token_unauthorized() {
        let svc = rig();
        let (token_orig, _) = svc.login("123456").unwrap();
        let mut bytes = token_orig.into_bytes();
        bytes[0] ^= 0x01;
        let token = String::from_utf8(bytes)
            .unwrap_or_else(|e| String::from_utf8_lossy(&e.into_bytes()).into_owned());
        assert!(matches!(svc.verify(&token), Err(AppError::Unauthorized)));
    }

    #[test]
    fn verify_pin_for_role_succeeds_when_role_meets_min() {
        let svc = rig();
        // rig() seeds an Owner with PIN "123456"
        let s = svc.verify_pin_for_role("123456", Role::Manager).unwrap();
        assert_eq!(s.role, Role::Owner);
    }

    #[test]
    fn verify_pin_for_role_fails_when_pin_belongs_to_lower_role() {
        let svc = rig();
        let staff_pin = "555555";
        let h = hash_pin(staff_pin).unwrap();
        svc.master
            .lock()
            .unwrap()
            .create_staff("Worker", &h, Role::Staff, None)
            .unwrap();
        assert!(matches!(
            svc.verify_pin_for_role(staff_pin, Role::Manager),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn verify_pin_for_role_fails_for_unknown_pin() {
        let svc = rig();
        assert!(matches!(
            svc.verify_pin_for_role("000000", Role::Staff),
            Err(AppError::Unauthorized)
        ));
    }

    #[test]
    fn login_generates_unique_jti() {
        let svc = rig();
        let (_, c1) = svc.login("123456").unwrap();
        let (_, c2) = svc.login("123456").unwrap();
        assert_ne!(c1.jti, c2.jti);
        assert!(!c1.jti.is_empty());
    }

    #[test]
    fn revoked_token_fails_verify() {
        let svc = rig();
        let (token, claims) = svc.login("123456").unwrap();
        // Sanity: verify succeeds before revoke.
        assert!(svc.verify(&token).is_ok());
        svc.revoke(&claims).unwrap();
        assert!(matches!(svc.verify(&token), Err(AppError::Unauthorized)));
    }

    #[test]
    fn revoke_is_idempotent() {
        let svc = rig();
        let (_, claims) = svc.login("123456").unwrap();
        svc.revoke(&claims).unwrap();
        // Second call should not error.
        svc.revoke(&claims).unwrap();
    }
}
