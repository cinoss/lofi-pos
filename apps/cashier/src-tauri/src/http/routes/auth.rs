use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::error::AppError;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub pin: String,
}

#[derive(Debug, Serialize)]
pub struct LoginOutput {
    pub token: String,
    pub claims: TokenClaims,
}

/// Unrated routes — /auth/logout and /auth/me. The /auth/login route is
/// mounted separately by `http::server::build_router` so its rate-limit
/// layer doesn't apply to logout/me.
pub fn router_unrated() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
}

/// Public handler — wired into the rate-limited slice in `server::build_router`.
pub async fn login_handler(
    State(state): State<Arc<AppState>>,
    Json(input): Json<LoginInput>,
) -> Result<Json<LoginOutput>, AppErrorResponse> {
    let auth = state.auth.clone();
    let (token, claims) = tokio::task::spawn_blocking(move || auth.login(&input.pin))
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(LoginOutput { token, claims }))
}

async fn me(AuthCtx(claims): AuthCtx) -> Json<TokenClaims> {
    Json(claims)
}

/// Revoke the calling token by adding its jti to the denylist. Subsequent
/// `verify` calls reject the token. No-op for pre-jti tokens.
async fn logout(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<axum::http::StatusCode, AppErrorResponse> {
    let auth = state.auth.clone();
    tokio::task::spawn_blocking(move || auth.revoke(&claims))
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
