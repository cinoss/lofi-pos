pub mod acl;
pub mod app_state;
pub mod auth;
pub mod bootstrap;
pub mod business_day;
pub mod crypto;
pub mod domain;
pub mod error;
pub mod http;
pub mod keychain;
pub mod print;
pub mod services;
pub mod store;
pub mod time;

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

            let event_service = services::event_service::EventService {
                master: master.clone(),
                events: events.clone(),
                kek: kek.clone(),
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

            let app_state = Arc::new(app_state::AppState {
                kek,
                master,
                events,
                clock,
                auth,
                commands: commands_svc,
                store,
                settings,
                broadcast_tx,
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
