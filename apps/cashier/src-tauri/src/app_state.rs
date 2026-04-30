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
    pub venue_name: String,
    pub venue_address: String,
    pub venue_phone: String,
    pub currency: String,
    pub locale: String,
    pub tax_id: String,
    pub receipt_footer: String,
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
        // Venue identity rows (added in 0009_venue_settings.sql). Defaults to
        // empty string when missing so old DBs missing the migration don't
        // hard-fail before we run it.
        fn get_or_empty(master: &Master, key: &str) -> AppResult<String> {
            Ok(master.get_setting(key)?.unwrap_or_default())
        }
        let venue_name = get_or_empty(master, "venue_name")?;
        let venue_address = get_or_empty(master, "venue_address")?;
        let venue_phone = get_or_empty(master, "venue_phone")?;
        let currency = get_or_empty(master, "currency")?;
        let locale = get_or_empty(master, "locale")?;
        let tax_id = get_or_empty(master, "tax_id")?;
        let receipt_footer = get_or_empty(master, "receipt_footer")?;
        Ok(Self {
            business_day_cutoff_hour: cutoff,
            business_day_tz: tz,
            discount_threshold_pct,
            cancel_grace_minutes,
            idle_lock_minutes,
            http_port,
            venue_name,
            venue_address,
            venue_phone,
            currency,
            locale,
            tax_id,
            receipt_footer,
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
    /// In-RAM seed cache populated at startup from the bouncer. Cashier
    /// startup hard-fails if the bouncer is unreachable, so this is always
    /// populated with whatever seeds the bouncer returned.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_load_returns_default_venue_values_after_migration() {
        let m = Master::open_in_memory().unwrap();
        let s = Settings::load(&m).unwrap();
        assert_eq!(s.venue_name, "");
        assert_eq!(s.venue_address, "");
        assert_eq!(s.venue_phone, "");
        assert_eq!(s.currency, "VND");
        assert_eq!(s.locale, "vi-VN");
        assert_eq!(s.tax_id, "");
        assert_eq!(s.receipt_footer, "");
    }
}
