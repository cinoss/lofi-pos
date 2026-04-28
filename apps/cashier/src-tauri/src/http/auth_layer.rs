use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::error::AppError;
use crate::http::error_layer::AppErrorResponse;
use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::sync::Arc;

/// Extracted from incoming requests via the `Authorization: Bearer <token>`
/// header. No cookie fallback by design — cookies enable CSRF when the
/// cashier API is reachable from a browser on the same LAN.
pub struct AuthCtx(pub TokenClaims);

/// Pure parsing helper, isolated for unit tests. Returns the bare token
/// substring or `Unauthorized` if the header is missing/malformed.
pub(crate) fn parse_bearer(header: Option<&str>) -> Result<&str, AppError> {
    header
        .and_then(|s| s.strip_prefix("Bearer "))
        .filter(|s| !s.is_empty())
        .ok_or(AppError::Unauthorized)
}

#[async_trait]
impl FromRequestParts<Arc<AppState>> for AuthCtx {
    type Rejection = AppErrorResponse;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let header = parts
            .headers
            .get("authorization")
            .and_then(|h| h.to_str().ok());
        let token = parse_bearer(header).map_err(AppErrorResponse)?;
        let claims = state.auth.verify(token).map_err(AppErrorResponse)?;
        Ok(AuthCtx(claims))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_bearer_present_returns_token() {
        assert_eq!(
            parse_bearer(Some("Bearer abc.def.ghi")).unwrap(),
            "abc.def.ghi"
        );
    }

    #[test]
    fn parse_bearer_missing_returns_unauthorized() {
        let err = parse_bearer(None).unwrap_err();
        assert!(matches!(err, AppError::Unauthorized));
    }

    #[test]
    fn parse_bearer_malformed_returns_unauthorized() {
        // No "Bearer " prefix
        assert!(matches!(
            parse_bearer(Some("Basic xyz")).unwrap_err(),
            AppError::Unauthorized
        ));
        // "Bearer " present but token empty
        assert!(matches!(
            parse_bearer(Some("Bearer ")).unwrap_err(),
            AppError::Unauthorized
        ));
    }
}
