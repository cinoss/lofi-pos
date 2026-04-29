use axum::Json;
use serde::Serialize;

#[derive(Serialize)]
pub struct Seed {
    pub id: String,
    pub label: String,
    pub default: bool,
    pub seed_hex: String,
}

pub async fn list() -> Json<Vec<Seed>> {
    Json(vec![Seed {
        id: "dev-default".into(),
        label: "Development default".into(),
        default: true,
        seed_hex: hex::encode(blake3::hash(b"lofi-pos-dev-seed-2026").as_bytes()),
    }])
}
