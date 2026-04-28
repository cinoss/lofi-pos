pub mod acl;
pub mod app_state;
pub mod auth;
pub mod bootstrap;
pub mod business_day;
pub mod cli;
pub mod crypto;
pub mod domain;
pub mod eod;
pub mod error;
pub mod http;
pub mod keychain;
pub mod print;
pub mod rotation;
pub mod services;
pub mod store;
pub mod time;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use tauri::Manager;

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .map_err(|e| crate::error::AppError::Config(format!("app_data_dir: {e}")))?;
            std::fs::create_dir_all(&data_dir)?;

            let ks = keychain::OsKeyStore::new(keychain::SERVICE);
            let kek = Arc::new(bootstrap::load_or_init_kek(&ks)?);

            let master_path = data_dir.join("master.db");
            let master = Arc::new(Mutex::new(store::master::Master::open(&master_path)?));
            tracing::info!(?master_path, "master db opened");

            let events_path = data_dir.join("events.db");
            let events = Arc::new(store::events::EventStore::open(&events_path)?);
            tracing::info!(?events_path, "events db opened");

            let clock: Arc<dyn time::Clock> = Arc::new(time::SystemClock);

            // Load TZ + cutoff from settings
            let (cutoff_hour, tz) = load_business_day_settings(&master.lock().unwrap())?;

            let key_manager = Arc::new(services::key_manager::KeyManager::new(
                master.clone(),
                kek.clone(),
            ));

            let event_service = services::event_service::EventService {
                events: events.clone(),
                key_manager: key_manager.clone(),
                clock: clock.clone(),
                cutoff_hour,
                tz,
            };

            let auth_signing = Arc::new(bootstrap::load_or_init_auth_signing(&ks)?);
            let auth = auth::AuthService {
                master: master.clone(),
                clock: clock.clone(),
                signing_key: auth_signing,
            };
            let auth_arc = Arc::new(auth.clone());

            let store = Arc::new(store::aggregate_store::AggregateStore::new());

            // Warm up from disk BEFORE managing AppState so the first
            // request sees a fully populated cache.
            let stats = store.warm_up(
                &master.lock().unwrap(),
                &events,
                &kek,
                &*clock,
                tz,
                cutoff_hour,
            )?;
            tracing::info!(?stats, "aggregate store warm-up complete");

            let settings = Arc::new(app_state::Settings::load(&master.lock().unwrap())?);
            let (broadcast_tx, _) = tokio::sync::broadcast::channel(256);

            let idem_lock = Arc::new(services::locking::KeyMutex::new());
            let agg_lock = Arc::new(services::locking::KeyMutex::new());
            let commands_svc = services::command_service::CommandService {
                master: master.clone(),
                events: events.clone(),
                event_service,
                clock: clock.clone(),
                auth: auth_arc,
                idem_lock,
                agg_lock,
                store: store.clone(),
                broadcast_tx: broadcast_tx.clone(),
            };

            // Plan F: reports go under <app_data_dir>/reports.
            let reports_dir = data_dir.join("reports");

            // Plan F: serve the admin SPA at /ui/admin/* from this directory.
            // Order of resolution:
            //   1. `LOFI_ADMIN_DIST` env var (dev override)
            //   2. Tauri resource_dir + "admin/dist" (prod bundle)
            //   3. <workspace>/apps/admin/dist (cargo run / dev fallback)
            // If the chosen path does not exist the static handler simply
            // 404s — `serve` logs a warning at startup so it's visible.
            let admin_dist: PathBuf = if let Ok(p) = std::env::var("LOFI_ADMIN_DIST") {
                PathBuf::from(p)
            } else if let Ok(res) = app.path().resource_dir() {
                res.join("admin").join("dist")
            } else {
                PathBuf::from("apps/admin/dist")
            };

            let app_state = Arc::new(app_state::AppState {
                kek,
                master,
                events,
                key_manager,
                clock,
                auth,
                commands: commands_svc,
                store,
                settings,
                broadcast_tx,
                reports_dir,
                admin_dist,
            });

            // Hand a clone to Tauri so future Rust-only callers (e.g., menu
            // handlers) can reach state; the UI itself talks HTTP.
            app.manage(app_state.clone());

            // Spawn HTTP server on Tauri's inherited Tokio runtime.
            let http_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = http::server::serve(http_state).await {
                    tracing::error!(?e, "http server exited with error");
                }
            });

            // Plan F: spawn the EOD scheduler on the same runtime. Performs
            // startup catch-up immediately, then loops on next_cutoff.
            eod::scheduler::spawn(app_state.clone());

            // UTC key rotation scheduler. Independent of EOD: ensures today's
            // DEK exists and prunes any DEK older than 3 UTC days.
            rotation::spawn(app_state.clone());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn load_business_day_settings(
    master: &store::master::Master,
) -> crate::error::AppResult<(u32, chrono::FixedOffset)> {
    let cutoff = master
        .get_setting("business_day_cutoff_hour")?
        .ok_or_else(|| crate::error::AppError::Config("business_day_cutoff_hour missing".into()))?
        .parse::<u32>()
        .map_err(|e| crate::error::AppError::Config(format!("cutoff parse: {e}")))?;
    let offset = master
        .get_setting("business_day_tz_offset_seconds")?
        .ok_or_else(|| {
            crate::error::AppError::Config("business_day_tz_offset_seconds missing".into())
        })?
        .parse::<i32>()
        .map_err(|e| crate::error::AppError::Config(format!("tz parse: {e}")))?;
    let tz = chrono::FixedOffset::east_opt(offset)
        .ok_or_else(|| crate::error::AppError::Config(format!("invalid tz offset: {offset}")))?;
    Ok((cutoff, tz))
}
