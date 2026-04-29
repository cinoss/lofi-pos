use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct HealthResp {
    pub ok: bool,
}

pub async fn get() -> Json<HealthResp> {
    Json(HealthResp { ok: true })
}
