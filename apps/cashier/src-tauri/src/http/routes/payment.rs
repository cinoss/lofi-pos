use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::DomainEvent;
use crate::domain::session::SessionState;
use crate::error::AppError;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct TakePaymentInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub subtotal: i64,
    pub discount_pct: u32,
    pub vat_pct: u32,
    pub total: i64,
    pub method: String,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/sessions/:id/payment", post(take_payment))
}

async fn take_payment(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(session_id): Path<String>,
    Json(input): Json<TakePaymentInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let threshold = s.settings.discount_threshold_pct;
        let action = if input.discount_pct == 0 {
            Action::TakePayment
        } else if input.discount_pct <= threshold {
            Action::ApplyDiscountSmall
        } else {
            Action::ApplyDiscountLarge
        };
        let ctx = PolicyCtx {
            discount_pct: Some(input.discount_pct),
            discount_threshold_pct: threshold,
            ..PolicyCtx::default()
        };
        // CONTRACT: aggregate_id = session_id so the duplicate-payment validation check works.
        let event = DomainEvent::PaymentTaken {
            session_id: session_id.clone(),
            subtotal: input.subtotal,
            discount_pct: input.discount_pct,
            vat_pct: input.vat_pct,
            total: input.total,
            method: input.method,
        };
        let (proj, _) = s.commands.execute(
            &claims,
            action,
            ctx,
            &input.idempotency_key,
            "take_payment",
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
