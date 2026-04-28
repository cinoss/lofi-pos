//! Stand-alone CLI entry points (no Tauri / no axum). Used by `main.rs`
//! to support `cashier eod-now [day]` for manual EOD runs.

use crate::app_state::{AppState, Settings};
use crate::auth::AuthService;
use crate::bootstrap;
use crate::eod::{business_day::business_day_for, business_day::Cfg, runner::run_eod};
use crate::keychain;
use crate::services::command_service::CommandService;
use crate::services::event_service::EventService;
use crate::services::locking::KeyMutex;
use crate::store::aggregate_store::AggregateStore;
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::{Clock, SystemClock};
use chrono::{Duration, FixedOffset};
use std::env;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Run the EOD pipeline for `day` (or yesterday if `day` is None).
/// Pure stand-alone path: opens master.db and events.db at the conventional
/// platform paths, builds a minimal AppState shell, invokes `run_eod`, and
/// returns. No tokio runtime needed beyond the function body.
pub fn run_eod_now(day: Option<String>) -> Result<(), Box<dyn std::error::Error>> {
    let identifier = "com.lofi-pos.cashier";
    let data_dir: PathBuf = if cfg!(target_os = "macos") {
        let home = env::var("HOME")?;
        PathBuf::from(home)
            .join("Library/Application Support")
            .join(identifier)
    } else if cfg!(target_os = "windows") {
        let appdata = env::var("APPDATA")?;
        PathBuf::from(appdata).join(identifier)
    } else {
        let home = env::var("HOME")?;
        PathBuf::from(home).join(".local/share").join(identifier)
    };

    let master_path = data_dir.join("master.db");
    let events_path = data_dir.join("events.db");
    if !master_path.exists() {
        return Err(format!("master.db not found at {master_path:?}").into());
    }

    let ks = keychain::OsKeyStore::new(keychain::SERVICE);
    let kek = Arc::new(bootstrap::load_or_init_kek(&ks)?);
    let master = Arc::new(Mutex::new(Master::open(&master_path)?));
    let events = Arc::new(EventStore::open(&events_path)?);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);

    let settings = Arc::new(Settings::load(&master.lock().unwrap())?);
    let cfg = Cfg {
        cutoff_hour: settings.business_day_cutoff_hour,
        tz_offset_seconds: settings.business_day_tz.local_minus_utc(),
    };

    let day = day.unwrap_or_else(|| {
        let today_ms = clock.now_ms();
        // Yesterday's business day.
        let yesterday_ms = today_ms - Duration::days(1).num_milliseconds();
        business_day_for(yesterday_ms, cfg)
    });

    let event_service = EventService {
        master: master.clone(),
        events: events.clone(),
        kek: kek.clone(),
        clock: clock.clone(),
        cutoff_hour: cfg.cutoff_hour,
        tz: FixedOffset::east_opt(cfg.tz_offset_seconds).unwrap(),
    };
    let auth_signing = Arc::new(bootstrap::load_or_init_auth_signing(&ks)?);
    let auth = AuthService {
        master: master.clone(),
        clock: clock.clone(),
        signing_key: auth_signing,
    };
    let store = Arc::new(AggregateStore::new());
    let (broadcast_tx, _rx) = tokio::sync::broadcast::channel(16);
    let commands = CommandService {
        master: master.clone(),
        events: events.clone(),
        event_service,
        clock: clock.clone(),
        auth: Arc::new(auth.clone()),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
        store: store.clone(),
        broadcast_tx: broadcast_tx.clone(),
    };
    let app_state = AppState {
        kek,
        master,
        events,
        clock,
        auth,
        commands,
        store,
        settings,
        broadcast_tx,
        reports_dir: data_dir.join("reports"),
        admin_dist: data_dir.join("admin").join("dist"),
    };

    let r = run_eod(&app_state, &day)?;
    println!("eod-now: business_day={} status={}", r.business_day, r.status);
    Ok(())
}
