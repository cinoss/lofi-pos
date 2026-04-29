mod health;
mod printers;
mod reports;
mod seeds;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let app = axum::Router::new()
        .route("/health", axum::routing::get(health::get))
        .route("/seeds", axum::routing::get(seeds::list))
        .route("/printers", axum::routing::get(printers::list))
        .route("/print", axum::routing::post(printers::print))
        .route("/reports/eod", axum::routing::post(reports::eod));
    let addr = std::env::var("BOUNCER_BIND").unwrap_or_else(|_| "127.0.0.1:7879".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("bouncer-mock listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
