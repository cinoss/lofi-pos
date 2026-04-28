use crate::app_state::AppState;
use crate::error::AppError;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use crate::store::master::{Product, Spot};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct StaffOut {
    pub id: i64,
    pub name: String,
    pub role: String,
    pub team: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/staff", get(list_staff))
        .route("/spots", get(list_spots))
        .route("/products", get(list_products))
}

async fn list_staff(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<StaffOut>>, AppErrorResponse> {
    let master = state.master.clone();
    let staff = tokio::task::spawn_blocking(move || master.lock().unwrap().list_staff())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(
        staff
            .into_iter()
            .map(|s| StaffOut {
                id: s.id,
                name: s.name,
                role: s.role.as_str().into(),
                team: s.team,
            })
            .collect(),
    ))
}

async fn list_spots(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<Spot>>, AppErrorResponse> {
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_spots())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn list_products(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<Product>>, AppErrorResponse> {
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_products())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}
