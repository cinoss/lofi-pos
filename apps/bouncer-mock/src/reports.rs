use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct EodReq {
    pub business_day: String,
    pub generated_at: i64,
    pub report: serde_json::Value,
}

pub async fn eod(Json(req): Json<EodReq>) -> impl IntoResponse {
    if let Err(e) = std::fs::create_dir_all("./tmp/reports") {
        tracing::error!("mkdir reports: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"stored": false, "error": e.to_string()})));
    }
    // Sanitize business_day to safe filename chars
    let safe: String = req
        .business_day
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect();
    if safe.is_empty() {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({"stored": false, "error": "invalid business_day"})));
    }
    let path = format!("./tmp/reports/{}.json", safe);
    let body = serde_json::json!({
        "business_day": req.business_day,
        "generated_at": req.generated_at,
        "report": req.report,
    });
    match std::fs::write(&path, serde_json::to_vec_pretty(&body).unwrap()) {
        Ok(()) => (StatusCode::OK, Json(serde_json::json!({"stored": true}))),
        Err(e) => {
            tracing::error!("write report: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"stored": false, "error": e.to_string()})))
        }
    }
}
