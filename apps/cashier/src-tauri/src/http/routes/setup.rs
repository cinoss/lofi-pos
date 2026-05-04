//! First-run setup endpoints. UNAUTHENTICATED — these run before any Owner
//! exists, so no token can be issued. Both endpoints gate on
//! `compute_needs_setup`:
//!
//! - `GET /admin/setup-state` is always reachable; reports `needs_setup` plus
//!   the externally-reachable LAN URL of this cashier so a phone on the same
//!   Wi-Fi can be handed the setup link.
//! - `POST /admin/setup` succeeds only while `needs_setup == true`. Once an
//!   Owner exists AND `venue_name` is non-empty, it returns 409 Conflict.

use crate::app_state::AppState;
use crate::auth::pin::{hash_pin, MIN_PIN_LENGTH};
use crate::error::{AppError, AppResult};
use crate::http::error_layer::AppErrorResponse;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Serialize)]
pub struct SetupStateOut {
    pub needs_setup: bool,
    /// LAN-reachable URL of this cashier (e.g. http://192.168.1.45:7878). Falls
    /// back to localhost if no LAN IP could be resolved.
    pub lan_url: String,
}

#[derive(Debug, Deserialize)]
pub struct SetupBody {
    // Venue identity
    pub venue_name: String,
    pub venue_address: String,
    pub venue_phone: String,
    pub currency: String,
    pub locale: String,
    pub tax_id: String,
    pub receipt_footer: String,
    // Operational
    pub business_day_cutoff_hour: i64,
    pub business_day_tz_offset_seconds: i64,
    // Owner account
    pub owner_name: String,
    pub owner_pin: String,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/admin/setup-state", get(get_state))
        .route("/admin/setup", post(submit))
}

async fn get_state(
    State(state): State<Arc<AppState>>,
) -> Result<Json<SetupStateOut>, AppErrorResponse> {
    let master = state.master.clone();
    let needs = tokio::task::spawn_blocking(move || compute_needs_setup_master(&master))
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    let port = state.settings.http_port;
    let lan_url = crate::net::primary_lan_ipv4()
        .map(|ip| format!("http://{ip}:{port}"))
        .unwrap_or_else(|| format!("http://localhost:{port}"));
    Ok(Json(SetupStateOut {
        needs_setup: needs,
        lan_url,
    }))
}

async fn submit(
    State(state): State<Arc<AppState>>,
    Json(body): Json<SetupBody>,
) -> Result<StatusCode, AppErrorResponse> {
    // Pre-validate cheap stuff before grabbing the lock.
    if body.venue_name.trim().is_empty() {
        return Err(AppErrorResponse(AppError::Validation(
            "venue_name required".into(),
        )));
    }
    if body.owner_name.trim().is_empty() {
        return Err(AppErrorResponse(AppError::Validation(
            "owner_name required".into(),
        )));
    }
    if body.owner_pin.chars().count() < MIN_PIN_LENGTH {
        return Err(AppErrorResponse(AppError::Validation(format!(
            "owner_pin must be at least {MIN_PIN_LENGTH} characters"
        ))));
    }
    if !(0..=23).contains(&body.business_day_cutoff_hour) {
        return Err(AppErrorResponse(AppError::Validation(
            "business_day_cutoff_hour out of range".into(),
        )));
    }
    if body.currency.trim().is_empty() {
        return Err(AppErrorResponse(AppError::Validation(
            "currency required".into(),
        )));
    }
    if body.locale.trim().is_empty() {
        return Err(AppErrorResponse(AppError::Validation(
            "locale required".into(),
        )));
    }

    // PIN hashing happens off the lock — argon2 is intentionally slow.
    let pin_hash =
        tokio::task::spawn_blocking(move || -> AppResult<(SetupBody, String)> {
            let h = hash_pin(&body.owner_pin)?;
            Ok((body, h))
        })
        .await
        .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    let (body, pin_hash) = pin_hash;

    let master = state.master.clone();
    let now = state.clock.now_ms();
    tokio::task::spawn_blocking(move || -> AppResult<()> {
        let mut master = master.lock().unwrap();
        if !compute_needs_setup_conn(&master)? {
            return Err(AppError::Conflict("setup already complete".into()));
        }
        master.with_tx(|tx| {
            for (k, v) in &[
                ("venue_name", body.venue_name.as_str()),
                ("venue_address", body.venue_address.as_str()),
                ("venue_phone", body.venue_phone.as_str()),
                ("currency", body.currency.as_str()),
                ("locale", body.locale.as_str()),
                ("tax_id", body.tax_id.as_str()),
                ("receipt_footer", body.receipt_footer.as_str()),
            ] {
                tx.execute(
                    "INSERT INTO setting(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    rusqlite::params![k, v],
                )?;
            }
            let cutoff = body.business_day_cutoff_hour.to_string();
            let tz = body.business_day_tz_offset_seconds.to_string();
            for (k, v) in &[
                ("business_day_cutoff_hour", cutoff.as_str()),
                ("business_day_tz_offset_seconds", tz.as_str()),
            ] {
                tx.execute(
                    "INSERT INTO setting(key, value) VALUES (?1, ?2)
                     ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                    rusqlite::params![k, v],
                )?;
            }
            tx.execute(
                "INSERT INTO staff(name, pin_hash, role, team, created_at)
                 VALUES (?1, ?2, 'owner', NULL, ?3)",
                rusqlite::params![body.owner_name, pin_hash, now],
            )?;
            // Seed a "Room Time" product (kind=time) so merge can fold a
            // source's room-time charge into the target as a line item.
            // INSERT OR IGNORE keeps this defensive against re-runs.
            tx.execute(
                "INSERT OR IGNORE INTO product (name, price, route, kind) \
                 VALUES ('Room Time', 0, 'none', 'time')",
                [],
            )?;
            Ok(())
        })?;
        Ok(())
    })
    .await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;

    Ok(StatusCode::CREATED)
}

fn compute_needs_setup_master(
    master: &Arc<std::sync::Mutex<crate::store::master::Master>>,
) -> AppResult<bool> {
    let conn = master.lock().unwrap();
    compute_needs_setup_conn(&conn)
}

fn compute_needs_setup_conn(master: &crate::store::master::Master) -> AppResult<bool> {
    use crate::acl::Role;
    let staff = master.list_staff()?;
    let has_owner = staff.iter().any(|s| s.role == Role::Owner);
    if !has_owner {
        return Ok(true);
    }
    let venue = master.get_setting("venue_name")?.unwrap_or_default();
    Ok(venue.trim().is_empty())
}
