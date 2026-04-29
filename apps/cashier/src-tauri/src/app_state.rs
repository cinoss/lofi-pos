use crate::auth::AuthService;
use crate::bouncer::client::BouncerClient;
use crate::bouncer::seed_cache::SeedCache;
use crate::error::{AppError, AppResult};
use crate::services::command_service::CommandService;
use crate::services::key_manager::KeyManager;
use crate::store::aggregate_store::AggregateStore;
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::Clock;
use chrono::FixedOffset;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Settings cache. Loaded once at startup from `master.db`. Held inside
/// `AppState` as `Arc<Settings>` so cheap clones reach every async handler.
#[derive(Debug, Clone)]
pub struct Settings {
    pub business_day_cutoff_hour: u32,
    pub business_day_tz: FixedOffset,
    pub discount_threshold_pct: u32,
    pub cancel_grace_minutes: u32,
    pub idle_lock_minutes: u32,
    pub http_port: u16,
}

impl Settings {
    /// Read all settings from the master DB. All keys but `http_port` are
    /// REQUIRED (seeded by 0001_init.sql); `http_port` defaults to 7878.
    pub fn load(master: &Master) -> AppResult<Self> {
        fn req<T>(master: &Master, key: &str) -> AppResult<T>
        where
            T: std::str::FromStr,
            <T as std::str::FromStr>::Err: std::fmt::Display,
        {
            let s = master
                .get_setting(key)?
                .ok_or_else(|| AppError::Config(format!("setting missing: {key}")))?;
            s.parse::<T>()
                .map_err(|e| AppError::Config(format!("setting {key} parse: {e}")))
        }
        let cutoff: u32 = req(master, "business_day_cutoff_hour")?;
        let tz_seconds: i32 = req(master, "business_day_tz_offset_seconds")?;
        let tz = FixedOffset::east_opt(tz_seconds)
            .ok_or_else(|| AppError::Config(format!("bad tz offset: {tz_seconds}")))?;
        let discount_threshold_pct: u32 = req(master, "discount_threshold_pct")?;
        let cancel_grace_minutes: u32 = req(master, "cancel_grace_minutes")?;
        let idle_lock_minutes: u32 = req(master, "idle_lock_minutes")?;
        let http_port: u16 = master
            .get_setting("http_port")?
            .map(|s| s.parse().unwrap_or(7878))
            .unwrap_or(7878);
        Ok(Self {
            business_day_cutoff_hour: cutoff,
            business_day_tz: tz,
            discount_threshold_pct,
            cancel_grace_minutes,
            idle_lock_minutes,
            http_port,
        })
    }
}

/// Tauri-managed shared state. Held by the runtime for process lifetime;
/// dropped on shutdown. Seed material lives only in the bouncer-fetched
/// `SeedCache`, where each seed is wrapped in `zeroize::Zeroizing` so the
/// bytes are wiped from RAM when the cache drops.
///
/// Note: the canonical `EventService` instance lives inside `commands`
/// (`commands.event_service`); we deliberately do not store a duplicate here.
/// The `AggregateStore` is also held inside `commands`; the `store` field here
/// is a convenience clone for read-only consumers (e.g., HTTP handlers in E1).
pub struct AppState {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    /// Per-business-day DEK derivation, backed by the bouncer-fetched
    /// `SeedCache`. Shared with `EventService` for write/read paths.
    pub key_manager: Arc<KeyManager>,
    /// In-RAM seed cache populated at startup from the bouncer (or fallback
    /// only if the bouncer is unreachable). Exposed so handlers/UI can read
    /// `degraded` for a banner.
    pub seed_cache: Arc<SeedCache>,
    /// HTTP client for the bouncer sidecar. Used by EOD (`post_report`) and
    /// the print queue worker (`print`).
    pub bouncer: Arc<BouncerClient>,
    pub clock: Arc<dyn Clock>,
    pub auth: AuthService,
    pub commands: CommandService,
    pub store: Arc<AggregateStore>,
    pub settings: Arc<Settings>,
    pub broadcast_tx: tokio::sync::broadcast::Sender<crate::http::broadcast::EventNotice>,
    /// Filesystem path to the built `apps/admin` SPA. Served by axum at
    /// `/ui/admin/*`. May not exist (dev convenience): the static handler
    /// logs a warning and returns 404 in that case.
    pub admin_dist: PathBuf,
}
