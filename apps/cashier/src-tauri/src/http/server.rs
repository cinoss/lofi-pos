use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use axum::response::IntoResponse;
use axum::Json;
use axum::Router;
use serde_json::json;
use std::net::SocketAddr;
use std::sync::Arc;
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::trace::TraceLayer;

/// Build the full axum router for the cashier API.
///
/// Layers (outermost first, applied in this order so all routes get them):
/// - `TraceLayer` — request/response tracing.
/// - `RequestBodyLimitLayer` — 64 KiB cap (stops large-payload abuse).
/// - `CorsLayer` — `Any` origin/method/header. Safe because all auth is via
///   the `Authorization: Bearer …` header (no cookies, so a malicious page
///   on the staff LAN cannot ride a session via CSRF).
///
/// Per-route: `/auth/login` is rate-limited via `tower_governor`
/// (per source IP, ≈10/min with burst 3) and returns HTTP 429 once the
/// quota is exceeded. `/auth/logout` and `/auth/me` are NOT rate-limited
/// — they require an already-issued token, so they aren't useful brute
/// targets.
pub fn build_router(state: Arc<AppState>) -> Router {
    let admin_dist = state.admin_dist.clone();
    // 6 per second sustained × burst 3 ≈ 10 attempts/IP/minute. The crate
    // uses a token-bucket: `period` is the refill interval (one token per
    // 1/per_second seconds), `burst_size` is the bucket capacity.
    let governor_conf = GovernorConfigBuilder::default()
        .per_second(6)
        .burst_size(3)
        .error_handler(|_err| {
            // Render the 429 in the standard `AppErrorEnvelope` shape so
            // `ApiClient` in `packages/shared` can pattern-match on
            // `code == "rate_limited"` like any other typed error.
            // `error_handler` wants `http::Response<axum::body::Body>`,
            // which is exactly what `IntoResponse` produces here.
            (
                axum::http::StatusCode::TOO_MANY_REQUESTS,
                Json(json!({
                    "code": "rate_limited",
                    "message": "too many login attempts",
                })),
            )
                .into_response()
        })
        .finish()
        .expect("static governor config");
    let governor_conf = Arc::new(governor_conf);

    // Rate-limited slice — only /auth/login. Build it as its own router so
    // the layer applies *only* to that route.
    let login_only = Router::new()
        .route(
            "/auth/login",
            axum::routing::post(crate::http::routes::auth::login_handler),
        )
        .layer(GovernorLayer {
            config: governor_conf,
        });

    Router::new()
        .merge(login_only)
        .merge(crate::http::routes::auth::router_unrated())
        .merge(crate::http::routes::catalog::router())
        .merge(crate::http::routes::session::router())
        .merge(crate::http::routes::order::router())
        .merge(crate::http::routes::payment::router())
        .merge(crate::http::routes::ws::router())
        .merge(crate::http::routes::admin::router())
        .with_state(state)
        // Admin SPA static mount. Lives at `/ui/admin/*`; `/admin/*` is the
        // JSON API. SPA fallback inside `static_admin::router` returns
        // index.html so client-side routing works.
        .nest_service("/ui/admin", crate::http::static_admin::service(admin_dist))
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
        .layer(RequestBodyLimitLayer::new(64 * 1024))
        .layer(TraceLayer::new_for_http())
}

/// Bind `0.0.0.0:<settings.http_port>` and serve the router until the
/// process exits or `axum::serve` returns an error.
pub async fn serve(state: Arc<AppState>) -> AppResult<()> {
    let port = state.settings.http_port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    if !state.admin_dist.exists() {
        tracing::warn!(
            path = ?state.admin_dist,
            "admin_dist directory does not exist; /ui/admin/* will return 404",
        );
    }
    let router = build_router(state);
    tracing::info!(%addr, "axum http server listening");
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(AppError::Io)?;
    axum::serve(
        listener,
        router.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await
    .map_err(|e| AppError::Internal(format!("axum serve: {e}")))?;
    Ok(())
}
