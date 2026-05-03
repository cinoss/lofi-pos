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
    extract::Request,
    http::{HeaderMap, HeaderValue, StatusCode, Uri},
    response::Response,
    routing::any,
    Router,
};
use std::path::PathBuf;
use std::sync::OnceLock;
use tower_http::services::{ServeDir, ServeFile};

/// Cached at first mount call so the proxy handler doesn't re-read the env
/// var on every request, and so that `mount` can be called from a typed
/// `Router<Arc<AppState>>` chain without forcing the proxy state into the
/// shared AppState.
static UPSTREAM: OnceLock<String> = OnceLock::new();

/// Mount the admin SPA on the given top-level Router. Picks proxy vs static
/// based on `LOFI_ADMIN_DEV_URL`. We attach top-level routes (not `nest`)
/// because axum 0.7's `nest("/ui/admin", inner)` has an edge case where the
/// inner router's `fallback` doesn't fire for the trailing-slash-only case
/// (`/ui/admin/`) when the inner request path is empty/`/`.
pub fn mount<S: Clone + Send + Sync + 'static>(
    router: Router<S>,
    admin_dist: PathBuf,
) -> Router<S> {
    if let Some(upstream) = dev_upstream() {
        tracing::info!(upstream = %upstream, "admin SPA: proxying to vite dev");
        let _ = UPSTREAM.set(upstream);
        router
            .route("/ui/admin", any(proxy_handler))
            .route("/ui/admin/", any(proxy_handler))
            .route("/ui/admin/*path", any(proxy_handler))
    } else {
        let index = admin_dist.join("index.html");
        router.nest_service(
            "/ui/admin",
            ServeDir::new(admin_dist).fallback(ServeFile::new(index)),
        )
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

async fn proxy_handler(req: Request) -> Result<Response, StatusCode> {
    let upstream = UPSTREAM.get().ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    // Top-level routes preserve the full /ui/admin... path, so we forward
    // verbatim. Vite is configured with `base: "/ui/admin/"`. Bare /ui/admin
    // is normalized to /ui/admin/ so vite's index handler matches.
    let original_uri: &Uri = req.uri();
    let raw_path = original_uri.path();
    let path = if raw_path == "/ui/admin" {
        "/ui/admin/".to_string()
    } else {
        raw_path.to_string()
    };
    let query = original_uri.query().map(|q| format!("?{q}")).unwrap_or_default();
    let url = format!("{upstream}{path}{query}");

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
