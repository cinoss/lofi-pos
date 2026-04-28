//! Owner-only admin CRUD: spots, staff, products, settings.
//!
//! Every handler runs `policy::check` with the appropriate `Action`. Only
//! the `Allow` branch proceeds — `OverrideRequired` and `Deny` are both
//! folded to `AppError::Unauthorized` because admin pages don't surface a
//! PIN-override prompt; the caller is expected to log in directly as Owner.

use crate::acl::policy::{self, PolicyCtx};
use crate::acl::{Action, Role};
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::error::{AppError, AppResult};
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use crate::store::master::{Product, Spot, SpotKind};
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

/// Reject the request unless the actor is allowed to perform `action` outright
/// (no override-PIN flow on admin pages).
fn require(action: Action, claims: &TokenClaims) -> AppResult<()> {
    match policy::check(action, claims.role, PolicyCtx::default()) {
        policy::Decision::Allow => Ok(()),
        _ => Err(AppError::Unauthorized),
    }
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        // Spots — Action::SpotEdit
        .route("/admin/spots", get(list_spots).post(create_spot))
        .route(
            "/admin/spots/:id",
            put(update_spot).delete(delete_spot),
        )
        // Staff — Action::EditStaff
        .route("/admin/staff", get(list_staff).post(create_staff))
        .route(
            "/admin/staff/:id",
            put(update_staff).delete(delete_staff),
        )
        // Products — Action::EditMenu
        .route("/admin/products", get(list_products).post(create_product))
        .route(
            "/admin/products/:id",
            put(update_product).delete(delete_product),
        )
        // Settings — Action::EditSettings
        .route("/admin/settings", get(get_settings).put(update_settings))
}

// ---------- Spots ----------

#[derive(Debug, Deserialize)]
pub struct SpotInput {
    pub name: String,
    pub kind: SpotKind,
    pub hourly_rate: Option<i64>,
    pub parent_id: Option<i64>,
}

async fn list_spots(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<Json<Vec<Spot>>, AppErrorResponse> {
    require(Action::SpotEdit, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_spots())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn create_spot(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<SpotInput>,
) -> Result<(StatusCode, Json<Spot>), AppErrorResponse> {
    require(Action::SpotEdit, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let spot = tokio::task::spawn_blocking(move || -> AppResult<Spot> {
        let m = master.lock().unwrap();
        let id = m.create_spot(&input.name, input.kind, input.hourly_rate, input.parent_id)?;
        m.get_spot(id)?.ok_or(AppError::NotFound)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok((StatusCode::CREATED, Json(spot)))
}

async fn update_spot(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(id): Path<i64>,
    Json(input): Json<SpotInput>,
) -> Result<Json<Spot>, AppErrorResponse> {
    require(Action::SpotEdit, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let spot = tokio::task::spawn_blocking(move || -> AppResult<Spot> {
        let m = master.lock().unwrap();
        let updated =
            m.update_spot(id, &input.name, input.kind, input.hourly_rate, input.parent_id)?;
        if !updated {
            return Err(AppError::NotFound);
        }
        m.get_spot(id)?.ok_or(AppError::NotFound)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(spot))
}

async fn delete_spot(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppErrorResponse> {
    require(Action::SpotEdit, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let removed = tokio::task::spawn_blocking(move || master.lock().unwrap().delete_spot(id))
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppErrorResponse(AppError::NotFound))
    }
}

// ---------- Staff ----------

#[derive(Debug, Deserialize)]
pub struct StaffInput {
    pub name: String,
    /// Plaintext PIN; hashed with argon2 before insert. Min length matches
    /// `auth::pin::MIN_PIN_LENGTH` (validated by `hash_pin`).
    pub pin: String,
    pub role: Role,
    pub team: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct StaffUpdate {
    pub name: Option<String>,
    /// New PIN. Re-hashed when present; left untouched when absent.
    pub pin: Option<String>,
    pub role: Option<Role>,
    /// `Some(None)` clears the team; `None` leaves it untouched.
    pub team: Option<Option<String>>,
}

#[derive(Debug, Serialize)]
pub struct StaffOut {
    pub id: i64,
    pub name: String,
    pub role: Role,
    pub team: Option<String>,
}

fn staff_to_out(s: crate::store::master::Staff) -> StaffOut {
    StaffOut {
        id: s.id,
        name: s.name,
        role: s.role,
        team: s.team,
    }
}

async fn list_staff(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<Json<Vec<StaffOut>>, AppErrorResponse> {
    require(Action::EditStaff, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let staff = tokio::task::spawn_blocking(move || master.lock().unwrap().list_staff())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(staff.into_iter().map(staff_to_out).collect()))
}

async fn create_staff(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<StaffInput>,
) -> Result<(StatusCode, Json<StaffOut>), AppErrorResponse> {
    require(Action::EditStaff, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let out = tokio::task::spawn_blocking(move || -> AppResult<StaffOut> {
        let pin_hash = crate::auth::pin::hash_pin(&input.pin)?;
        let m = master.lock().unwrap();
        let id = m.create_staff(&input.name, &pin_hash, input.role, input.team.as_deref())?;
        let s = m.get_staff(id)?.ok_or(AppError::NotFound)?;
        Ok(staff_to_out(s))
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok((StatusCode::CREATED, Json(out)))
}

async fn update_staff(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(id): Path<i64>,
    Json(input): Json<StaffUpdate>,
) -> Result<Json<StaffOut>, AppErrorResponse> {
    require(Action::EditStaff, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let out = tokio::task::spawn_blocking(move || -> AppResult<StaffOut> {
        let pin_hash = match input.pin.as_deref() {
            Some(pin) => Some(crate::auth::pin::hash_pin(pin)?),
            None => None,
        };
        let m = master.lock().unwrap();
        let team_arg: Option<Option<&str>> = input
            .team
            .as_ref()
            .map(|t| t.as_deref());
        let updated = m.update_staff(
            id,
            input.name.as_deref(),
            pin_hash.as_deref(),
            input.role,
            team_arg,
        )?;
        if !updated {
            return Err(AppError::NotFound);
        }
        let s = m.get_staff(id)?.ok_or(AppError::NotFound)?;
        Ok(staff_to_out(s))
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(out))
}

async fn delete_staff(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppErrorResponse> {
    require(Action::EditStaff, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let removed = tokio::task::spawn_blocking(move || master.lock().unwrap().delete_staff(id))
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppErrorResponse(AppError::NotFound))
    }
}

// ---------- Products ----------

#[derive(Debug, Deserialize)]
pub struct ProductInput {
    pub name: String,
    pub price: i64,
    pub route: String,
    pub kind: String,
}

async fn list_products(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<Json<Vec<Product>>, AppErrorResponse> {
    require(Action::EditMenu, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_products())
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn create_product(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<ProductInput>,
) -> Result<(StatusCode, Json<Product>), AppErrorResponse> {
    require(Action::EditMenu, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let p = tokio::task::spawn_blocking(move || -> AppResult<Product> {
        let m = master.lock().unwrap();
        let id = m.create_product(&input.name, input.price, &input.route, &input.kind)?;
        m.get_product(id)?.ok_or(AppError::NotFound)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok((StatusCode::CREATED, Json(p)))
}

async fn update_product(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(id): Path<i64>,
    Json(input): Json<ProductInput>,
) -> Result<Json<Product>, AppErrorResponse> {
    require(Action::EditMenu, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let p = tokio::task::spawn_blocking(move || -> AppResult<Product> {
        let m = master.lock().unwrap();
        let updated = m.update_product(id, &input.name, input.price, &input.route, &input.kind)?;
        if !updated {
            return Err(AppError::NotFound);
        }
        m.get_product(id)?.ok_or(AppError::NotFound)
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(p))
}

async fn delete_product(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Path(id): Path<i64>,
) -> Result<StatusCode, AppErrorResponse> {
    require(Action::EditMenu, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let removed = tokio::task::spawn_blocking(move || master.lock().unwrap().delete_product(id))
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    if removed {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Err(AppErrorResponse(AppError::NotFound))
    }
}

// ---------- Settings ----------

#[derive(Debug, Serialize)]
pub struct SettingsOut {
    pub business_day_cutoff_hour: u32,
    pub business_day_tz_offset_seconds: i32,
    pub discount_threshold_pct: u32,
    pub cancel_grace_minutes: u32,
    pub idle_lock_minutes: u32,
}

#[derive(Debug, Deserialize)]
pub struct SettingsUpdate {
    pub business_day_cutoff_hour: Option<u32>,
    pub business_day_tz_offset_seconds: Option<i32>,
    pub discount_threshold_pct: Option<u32>,
    pub cancel_grace_minutes: Option<u32>,
    pub idle_lock_minutes: Option<u32>,
}

async fn get_settings(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<Json<SettingsOut>, AppErrorResponse> {
    require(Action::EditSettings, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let out = tokio::task::spawn_blocking(move || -> AppResult<SettingsOut> {
        let m = master.lock().unwrap();
        let s = crate::app_state::Settings::load(&m)?;
        Ok(SettingsOut {
            business_day_cutoff_hour: s.business_day_cutoff_hour,
            business_day_tz_offset_seconds: s.business_day_tz.local_minus_utc(),
            discount_threshold_pct: s.discount_threshold_pct,
            cancel_grace_minutes: s.cancel_grace_minutes,
            idle_lock_minutes: s.idle_lock_minutes,
        })
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(out))
}

async fn update_settings(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<SettingsUpdate>,
) -> Result<Json<SettingsOut>, AppErrorResponse> {
    require(Action::EditSettings, &claims).map_err(AppErrorResponse)?;
    let master = state.master.clone();
    let out = tokio::task::spawn_blocking(move || -> AppResult<SettingsOut> {
        let m = master.lock().unwrap();
        if let Some(v) = input.business_day_cutoff_hour {
            if v > 23 {
                return Err(AppError::Validation("business_day_cutoff_hour out of range".into()));
            }
            m.set_setting("business_day_cutoff_hour", &v.to_string())?;
        }
        if let Some(v) = input.business_day_tz_offset_seconds {
            m.set_setting("business_day_tz_offset_seconds", &v.to_string())?;
        }
        if let Some(v) = input.discount_threshold_pct {
            m.set_setting("discount_threshold_pct", &v.to_string())?;
        }
        if let Some(v) = input.cancel_grace_minutes {
            m.set_setting("cancel_grace_minutes", &v.to_string())?;
        }
        if let Some(v) = input.idle_lock_minutes {
            m.set_setting("idle_lock_minutes", &v.to_string())?;
        }
        let s = crate::app_state::Settings::load(&m)?;
        Ok(SettingsOut {
            business_day_cutoff_hour: s.business_day_cutoff_hour,
            business_day_tz_offset_seconds: s.business_day_tz.local_minus_utc(),
            discount_threshold_pct: s.discount_threshold_pct,
            cancel_grace_minutes: s.cancel_grace_minutes,
            idle_lock_minutes: s.idle_lock_minutes,
        })
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(out))
}
