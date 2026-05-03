use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::DomainEvent;
use crate::domain::session::SessionState;
use crate::error::AppError;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use crate::http::spot_helper::build_spot_ref;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct OpenSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub spot_id: i64,
    pub customer_label: Option<String>,
    pub team: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct CloseSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TransferSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub to_spot_id: i64,
}

#[derive(Debug, Deserialize)]
pub struct MergeSessionsInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    /// Target session that absorbs the sources; SessionMerged is written
    /// with this as its aggregate_id.
    pub into_session: String,
    pub sources: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct SplitSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub new_sessions: Vec<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", post(open_session))
        .route("/sessions/active", get(list_active))
        .route("/sessions/history", get(list_history))
        .route("/sessions/:id", get(get_session))
        .route("/sessions/:id/close", post(close_session))
        .route("/sessions/:id/transfer", post(transfer_session))
        .route("/sessions/merge", post(merge_sessions))
        .route("/sessions/:id/split", post(split_session))
}

async fn list_active(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<SessionState>>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || s.commands.list_active_sessions())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn list_history(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<SessionState>>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || s.commands.list_history_sessions())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn get_session(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
    Path(session_id): Path<String>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        s.commands
            .load_session(&session_id)?
            .ok_or(AppError::NotFound)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn open_session(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<OpenSessionInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let spot = s
            .master
            .lock()
            .unwrap()
            .get_spot(input.spot_id)?
            .ok_or(AppError::NotFound)?;
        let spot_ref = build_spot_ref(&s, spot)?;
        let session_id = Uuid::new_v4().to_string();
        let event = DomainEvent::SessionOpened {
            spot: spot_ref,
            opened_by: claims.staff_id,
            customer_label: input.customer_label,
            team: input.team,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::OpenSession,
            PolicyCtx::default(),
            &input.idempotency_key,
            "open_session",
            &session_id,
            event,
            input.override_pin.as_deref(),
            |c| c.load_session(&session_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn close_session(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(session_id): Path<String>,
    Json(input): Json<CloseSessionInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let event = DomainEvent::SessionClosed {
            closed_by: claims.staff_id,
            reason: input.reason,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::CloseSession,
            PolicyCtx::default(),
            &input.idempotency_key,
            "close_session",
            &session_id,
            event,
            input.override_pin.as_deref(),
            |c| c.load_session(&session_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn transfer_session(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(session_id): Path<String>,
    Json(input): Json<TransferSessionInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let from = s
            .commands
            .load_session(&session_id)?
            .ok_or(AppError::NotFound)?
            .spot;
        let to_spot = s
            .master
            .lock()
            .unwrap()
            .get_spot(input.to_spot_id)?
            .ok_or(AppError::NotFound)?;
        let to = build_spot_ref(&s, to_spot)?;
        let event = DomainEvent::SessionTransferred { from, to };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::TransferSession,
            PolicyCtx::default(),
            &input.idempotency_key,
            "transfer_session",
            &session_id,
            event,
            input.override_pin.as_deref(),
            |c| c.load_session(&session_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn merge_sessions(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<MergeSessionsInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let into_session = input.into_session.clone();
        let event = DomainEvent::SessionMerged {
            into_session: input.into_session.clone(),
            sources: input.sources,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::MergeSessions,
            PolicyCtx::default(),
            &input.idempotency_key,
            "merge_sessions",
            &into_session,
            event,
            input.override_pin.as_deref(),
            |c| c.load_session(&into_session)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn split_session(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(from_session): Path<String>,
    Json(input): Json<SplitSessionInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let event = DomainEvent::SessionSplit {
            from_session: from_session.clone(),
            new_sessions: input.new_sessions,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            Action::SplitSession,
            PolicyCtx::default(),
            &input.idempotency_key,
            "split_session",
            &from_session,
            event,
            input.override_pin.as_deref(),
            |c| c.load_session(&from_session)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}
