//! Shared test helpers for EOD module tests (builder/runner/scheduler).
//!
//! Builds a fully-wired in-memory `AppState` (master.db + events.db, mock
//! clock, throwaway broadcast channel) and exposes high-level "place an
//! order"/"take a payment" helpers so tests don't have to re-thread the
//! whole CommandService pipeline by hand.
//!
//! Not gated by `#[cfg(test)]` because the EOD integration smoke tests in
//! `tests/` link against the lib and need access. Adds no production cost
//! beyond a few hundred bytes.

#![allow(dead_code)]

use crate::acl::policy::PolicyCtx;
use crate::acl::{Action, Role};
use crate::app_state::{AppState, Settings};
use crate::auth::pin::hash_pin;
use crate::auth::token::TokenClaims;
use crate::auth::AuthService;
use crate::crypto::Kek;
use crate::domain::event::{DomainEvent, OrderItemSpec, Route};
use crate::domain::spot::SpotRef;
use crate::services::command_service::CommandService;
use crate::services::event_service::EventService;
use crate::services::locking::KeyMutex;
use crate::store::aggregate_store::AggregateStore;
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::test_support::MockClock;
use crate::time::Clock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Cutoff hour shared across all EOD tests.
pub const TEST_CUTOFF: u32 = 11;
/// +07:00, the production case.
pub const TEST_TZ_SECONDS: i32 = 7 * 3600;

/// In-memory AppState rig shared across the EOD tests. Owns the mock clock
/// so tests can advance time, and a tempdir scoped to its lifetime.
pub struct EodRig {
    pub state: Arc<AppState>,
    pub clock: Arc<MockClock>,
    /// Owner's bearer claims — every test command runs as Owner so ACL
    /// is never the thing under test here.
    pub owner: TokenClaims,
    _tmp: tempfile::TempDir,
}

/// Build a fully-wired in-memory AppState. Frozen clock at the given UTC
/// instant. Master DB pre-seeded with one Owner staff (PIN `999999`).
pub fn seed_app_state_at(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> EodRig {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let mock_clock = Arc::new(MockClock::at_ymd_hms(y, m, d, h, mi, s));
    let clock: Arc<dyn Clock> = mock_clock.clone();

    let key_manager = Arc::new(crate::services::key_manager::KeyManager::new(
        master.clone(),
        kek.clone(),
    ));
    let event_service = EventService {
        events: events.clone(),
        key_manager: key_manager.clone(),
        clock: clock.clone(),
        cutoff_hour: TEST_CUTOFF,
        tz: FixedOffset::east_opt(TEST_TZ_SECONDS).unwrap(),
    };

    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService {
        master: master.clone(),
        clock: clock.clone(),
        signing_key: signing,
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

    // Seed an owner so tests can grab a token.
    let pin_hash = hash_pin("999999").unwrap();
    let owner_id = master
        .lock()
        .unwrap()
        .create_staff("Owner", &pin_hash, Role::Owner, None)
        .unwrap();

    let settings = Arc::new(Settings::load(&master.lock().unwrap()).unwrap());

    let tmp = tempfile::tempdir().unwrap();
    let reports_dir = tmp.path().join("reports");
    let admin_dist = tmp.path().join("admin_dist");

    let state = Arc::new(AppState {
        kek,
        master,
        events,
        key_manager,
        clock,
        auth,
        commands,
        store,
        settings,
        broadcast_tx,
        reports_dir,
        admin_dist,
    });

    let owner = TokenClaims {
        staff_id: owner_id,
        role: Role::Owner,
        jti: "test-jti".into(),
        iat: 0,
        exp: i64::MAX,
    };

    EodRig {
        state,
        clock: mock_clock,
        owner,
        _tmp: tmp,
    }
}

fn room(id: i64) -> SpotRef {
    SpotRef::Room {
        id,
        name: format!("R{id}"),
        hourly_rate: 50_000,
    }
}

fn item(product_id: i64, qty: i64, unit_price: i64) -> OrderItemSpec {
    OrderItemSpec {
        product_id,
        product_name: format!("P{product_id}"),
        qty,
        unit_price,
        note: None,
        route: Route::Bar,
        recipe_snapshot: vec![],
    }
}

/// Place a single bar-route order (1 unit @ 50k) on a fresh session.
/// Runs the full command pipeline (ACL/idempotency/validation/encrypt/append).
pub fn place_test_order(rig: &EodRig) -> (String, String) {
    let session_id = Uuid::new_v4().to_string();
    let order_id = Uuid::new_v4().to_string();
    let cs = &rig.state.commands;
    cs.execute(
        &rig.owner,
        Action::OpenSession,
        PolicyCtx::default(),
        &format!("open-{}", session_id),
        "open_session",
        &session_id,
        DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: rig.owner.staff_id,
            customer_label: None,
            team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();
    cs.execute(
        &rig.owner,
        Action::PlaceOrder,
        PolicyCtx::default(),
        &format!("ord-{}", order_id),
        "place_order",
        &order_id,
        DomainEvent::OrderPlaced {
            session_id: session_id.clone(),
            order_id: order_id.clone(),
            items: vec![item(1, 1, 50_000)],
        },
        None,
        |c| c.load_order(&order_id).map(|o| o.unwrap()),
    )
    .unwrap();
    (session_id, order_id)
}

/// Open + order + pay + close. Returns the session_id.
pub fn take_test_payment(rig: &EodRig) -> String {
    let (session_id, _) = place_test_order(rig);
    let cs = &rig.state.commands;
    cs.execute(
        &rig.owner,
        Action::TakePayment,
        PolicyCtx::default(),
        &format!("pay-{}", session_id),
        "take_payment",
        &session_id,
        DomainEvent::PaymentTaken {
            session_id: session_id.clone(),
            subtotal: 50_000,
            discount_pct: 0,
            vat_pct: 0,
            total: 50_000,
            method: "cash".into(),
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();
    cs.execute(
        &rig.owner,
        Action::CloseSession,
        PolicyCtx::default(),
        &format!("close-{}", session_id),
        "close_session",
        &session_id,
        DomainEvent::SessionClosed {
            closed_by: rig.owner.staff_id,
            reason: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();
    session_id
}

/// `state.master` access for db assertions.
pub fn day_key_exists(state: &AppState, day: &str) -> bool {
    state
        .master
        .lock()
        .unwrap()
        .get_dek(day)
        .unwrap()
        .is_some()
}

pub fn daily_report_exists(state: &AppState, day: &str) -> bool {
    state
        .master
        .lock()
        .unwrap()
        .get_daily_report(day)
        .unwrap()
        .is_some()
}

pub fn eod_runs_status(state: &AppState, day: &str) -> String {
    state
        .master
        .lock()
        .unwrap()
        .get_eod_runs_status(day)
        .unwrap()
        .unwrap_or_default()
}

pub fn idempotency_exists(state: &AppState, key: &str) -> bool {
    state
        .master
        .lock()
        .unwrap()
        .get_idempotency(key)
        .unwrap()
        .is_some()
}

pub fn insert_idempotency(state: &AppState, key: &str, ts_ms: i64) {
    state
        .master
        .lock()
        .unwrap()
        .put_idempotency(key, "synthetic", "{}", ts_ms)
        .unwrap();
}

/// UTC ms helper for tests that name times in business-day terms.
pub fn ts_ms(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(y, m, d, h, mi, 0)
        .unwrap()
        .timestamp_millis()
}
