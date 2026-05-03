//! Serves the `apps/admin` SPA at `/ui/admin/*`.
//!
//! Two modes:
//! - **Static (production)**: serves files from `admin_dist/`. SPA fallback
//!   returns `index.html` (with HTTP 200, not 404 — `ServeDir::fallback`
//!   wraps cleanly; `not_found_service` would mark it 404 and break
//!   react-router hydration).
//! - **Dev proxy**: when `LOFI_ADMIN_DEV_URL` is set (or in debug builds by
//!   default), forwards each request to a running `vite dev` instance for
//!   the admin app (typically `http://localhost:1421`). HMR's WebSocket
//!   connects directly to vite (configured via `server.hmr.clientPort` in
//!   admin's vite config), so this proxy only handles plain HTTP.

use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, Uri},
    response::Response,
    routing::any,
    Router,
};
use std::path::PathBuf;
use std::sync::Arc;
use tower_http::services::{ServeDir, ServeFile};

/// Build an axum Router that mounts the admin SPA. Caller `nest`s it under
/// `/ui/admin`. Picks proxy vs static based on `LOFI_ADMIN_DEV_URL`.
pub fn router(admin_dist: PathBuf) -> Router {
    if let Some(upstream) = dev_upstream() {
        tracing::info!(upstream = %upstream, "admin SPA: proxying to vite dev");
        Router::new()
            .route("/", any(proxy_handler))
            .route("/*path", any(proxy_handler))
            .with_state(Arc::new(upstream))
    } else {
        let index = admin_dist.join("index.html");
        Router::new().fallback_service(ServeDir::new(admin_dist).fallback(ServeFile::new(index)))
    }
}

fn dev_upstream() -> Option<String> {
    // Explicit opt-in via env. `tauri:dev` script sets it; production
    // builds and `cargo test` (no env) fall through to static-disk mode.
    let url = std::env::var("LOFI_ADMIN_DEV_URL").ok()?;
    let trimmed = url.trim_end_matches('/').trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(trimmed.to_string())
}

async fn proxy_handler(
    State(upstream): State<Arc<String>>,
    req: Request,
) -> Result<Response, StatusCode> {
    // The router is mounted under /ui/admin/*; the upstream vite dev also
    // serves with `base: /ui/admin/`, so the request URI's full path (with
    // /ui/admin/ prefix) maps 1:1.
    let original_uri: &Uri = req.uri();
    let path_and_query = original_uri
        .path_and_query()
        .map(|x| x.as_str())
        .unwrap_or("");
    let url = format!("{upstream}{path_and_query}");

    let method = req.method().clone();
    let headers = req.headers().clone();
    let body_bytes = axum::body::to_bytes(req.into_body(), 16 * 1024 * 1024)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;

    let upstream_resp = tokio::task::spawn_blocking(move || {
        let client = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(std::time::Duration::from_secs(15))
            .build()
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let mut req_builder = client.request(method, &url);
        for (name, value) in &headers {
            // Skip hop-by-hop headers; reqwest will set Host from URL.
            let n = name.as_str();
            if n.eq_ignore_ascii_case("host") || n.eq_ignore_ascii_case("content-length") {
                continue;
            }
            req_builder = req_builder.header(name.clone(), value.clone());
        }
        if !body_bytes.is_empty() {
            req_builder = req_builder.body(body_bytes.to_vec());
        }
        req_builder.send().map_err(|_| StatusCode::BAD_GATEWAY)
    })
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)??;

    let status = upstream_resp.status();
    let upstream_headers = upstream_resp.headers().clone();
    let body = tokio::task::spawn_blocking(move || upstream_resp.bytes())
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map_err(|_| StatusCode::BAD_GATEWAY)?;

    let mut resp = Response::builder().status(StatusCode::from_u16(status.as_u16()).unwrap());
    let resp_headers: &mut HeaderMap = resp.headers_mut().unwrap();
    for (name, value) in upstream_headers.iter() {
        let n = name.as_str();
        // Drop hop-by-hop headers; let axum set its own.
        if matches!(
            n.to_ascii_lowercase().as_str(),
            "transfer-encoding" | "connection" | "keep-alive"
        ) {
            continue;
        }
        if let (Ok(hn), Ok(hv)) = (
            axum::http::HeaderName::from_bytes(name.as_str().as_bytes()),
            HeaderValue::from_bytes(value.as_bytes()),
        ) {
            resp_headers.insert(hn, hv);
        }
    }
    Ok(resp.body(Body::from(body)).unwrap())
}
