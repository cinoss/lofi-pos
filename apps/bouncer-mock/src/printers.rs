use axum::http::StatusCode;
use axum::{Json, response::IntoResponse};
use serde::{Deserialize, Serialize};
use std::io::Write;

#[derive(Serialize)]
pub struct Printer {
    pub id: String,
    pub label: String,
}

pub async fn list() -> Json<Vec<Printer>> {
    Json(vec![Printer {
        id: "default".into(),
        label: "Default printer (mock)".into(),
    }])
}

#[derive(Deserialize)]
pub struct PrintReq {
    pub kind: String,
    pub payload: serde_json::Value,
    pub target_printer_id: Option<String>,
}

pub async fn print(Json(req): Json<PrintReq>) -> impl IntoResponse {
    if let Err(e) = std::fs::create_dir_all("./tmp") {
        tracing::error!("mkdir tmp: {e}");
        return (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"queued": false, "error": e.to_string()})));
    }
    let line = serde_json::json!({
        "kind": req.kind,
        "target_printer_id": req.target_printer_id,
        "payload": req.payload,
    });
    let res = (|| -> std::io::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open("./tmp/prints.log")?;
        writeln!(f, "{}", line)?;
        Ok(())
    })();
    match res {
        Ok(()) => (StatusCode::ACCEPTED, Json(serde_json::json!({"queued": true}))),
        Err(e) => {
            tracing::error!("print log write: {e}");
            (StatusCode::INTERNAL_SERVER_ERROR, Json(serde_json::json!({"queued": false, "error": e.to_string()})))
        }
    }
}
