//! Read-only report endpoints. Mounted under `/admin/reports/*` so the
//! admin SPA can browse historical EOD output.
//!
//! ACL: Manager-and-up via [`Action::ViewReports`]. Reports are aggregate
//! financial data — Manager needs them for daily ops, Owner edits them.

use crate::acl::policy::{self, PolicyCtx};
use crate::acl::Action;
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::error::{AppError, AppResult};
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use crate::store::master::DailyReportRow;
use axum::extract::{Path, State};
use axum::routing::get;
use axum::{Json, Router};
use std::sync::Arc;

fn require_view(claims: &TokenClaims) -> AppResult<()> {
    match policy::check(Action::ViewReports, claims.role, PolicyCtx::default()) {
        policy::Decision::Allow => Ok(()),
        _ => Err(AppError::Unauthorized),
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/reports", get(list_reports))
        .route("/admin/reports/:business_day", get(get_report))
}

async fn list_reports(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<Json<Vec<DailyReportRow>>, AppErrorResponse> {
    require_view(&claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_daily_reports())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn get_report(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(business_day): Path<String>,
) -> Result<Json<DailyReportRow>, AppErrorResponse> {
    require_view(&claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || {
        master.lock().unwrap().get_daily_report(&business_day)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?
    .ok_or(AppErrorResponse(AppError::NotFound))?;
    Ok(Json(r))
}
