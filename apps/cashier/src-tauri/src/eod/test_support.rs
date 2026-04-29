//! Shared test helpers for EOD module tests (builder/runner/scheduler).
//!
//! Builds a fully-wired in-memory `AppState` (master.db + events.db, mock
//! clock, in-process bouncer stub) and exposes high-level "place an order"
//! helpers so tests don't have to re-thread the whole CommandService
//! pipeline by hand.

#![allow(dead_code)]

use crate::acl::policy::PolicyCtx;
use crate::acl::{Action, Role};
use crate::app_state::{AppState, Settings};
use crate::auth::pin::hash_pin;
use crate::auth::token::TokenClaims;
use crate::auth::AuthService;
use crate::bouncer::client::BouncerClient;
use crate::bouncer::seed_cache::SeedCache;
use crate::domain::event::{DomainEvent, OrderItemSpec, Route};
use crate::domain::spot::SpotRef;
use crate::services::command_service::CommandService;
use crate::services::event_service::EventService;
use crate::services::key_manager::KeyManager;
use crate::services::locking::KeyMutex;
use crate::store::aggregate_store::AggregateStore;
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::test_support::MockClock;
use crate::time::Clock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex, OnceLock};
use uuid::Uuid;

/// Cutoff hour shared across all EOD tests.
pub const TEST_CUTOFF: u32 = 11;
/// +07:00, the production case.
pub const TEST_TZ_SECONDS: i32 = 7 * 3600;

/// Local-process bouncer stub URL. Started lazily by `stub_bouncer_url`.
static STUB_URL: OnceLock<String> = OnceLock::new();

/// Spawn a single in-process axum stub on a random port and return its base
/// URL. Subsequent calls return the same URL (the stub stays alive for the
/// process lifetime). The stub accepts everything and returns 2xx.
pub fn stub_bouncer_url() -> String {
    STUB_URL
        .get_or_init(|| {
            // Bind synchronously on a dedicated runtime in its own thread.
            let (tx, rx) = std::sync::mpsc::channel::<String>();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    use axum::routing::{get, post};
                    use axum::Json;
                    let app = axum::Router::new()
                        .route("/health", get(|| async { Json(serde_json::json!({"ok":true})) }))
                        .route(
                            "/seeds",
                            get(|| async {
                                Json(serde_json::json!([{
                                    "id": "stub-default",
                                    "label": "Stub default",
                                    "default": true,
                                    "seed_hex": "0101010101010101010101010101010101010101010101010101010101010101"
                                }]))
                            }),
                        )
                        .route(
                            "/print",
                            post(|axum::Json(_): axum::Json<serde_json::Value>| async {
                                Json(serde_json::json!({"queued": true}))
                            }),
                        )
                        .route(
                            "/reports/eod",
                            post(|axum::Json(_): axum::Json<serde_json::Value>| async {
                                Json(serde_json::json!({"stored": true}))
                            }),
                        );
                    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                    let addr = listener.local_addr().unwrap();
                    tx.send(format!("http://{addr}")).unwrap();
                    axum::serve(listener, app).await.unwrap();
                });
            });
            rx.recv().unwrap()
        })
        .clone()
}

pub struct EodRig {
    pub state: Arc<AppState>,
    pub clock: Arc<MockClock>,
    pub owner: TokenClaims,
}

fn build_state(
    y: i32,
    m: u32,
    d: u32,
    h: u32,
    mi: u32,
    s: u32,
    bouncer_url: String,
) -> EodRig {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let mock_clock = Arc::new(MockClock::at_ymd_hms(y, m, d, h, mi, s));
    let clock: Arc<dyn Clock> = mock_clock.clone();

    let bouncer = Arc::new(BouncerClient::new(bouncer_url));
    let seed_cache = Arc::new(SeedCache::from_seeds(
        "test",
        vec![("test".into(), [42u8; 32])],
    ));
    let key_manager = Arc::new(KeyManager::new(seed_cache.clone()));

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

    let pin_hash = hash_pin("999999").unwrap();
    let owner_id = master
        .lock()
        .unwrap()
        .create_staff("Owner", &pin_hash, Role::Owner, None)
        .unwrap();

    let settings = Arc::new(Settings::load(&master.lock().unwrap()).unwrap());

    let tmp = tempfile::tempdir().unwrap();
    let admin_dist = tmp.path().join("admin_dist");

    let state = Arc::new(AppState {
        master,
        events,
        key_manager,
        seed_cache,
        bouncer,
        clock,
        auth,
        commands,
        store,
        settings,
        broadcast_tx,
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
    }
}

/// Build an in-memory AppState wired against a shared in-process bouncer
/// stub that accepts everything.
pub fn seed_app_state_at(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> EodRig {
    build_state(y, m, d, h, mi, s, stub_bouncer_url())
}

/// Like `seed_app_state_at` but the bouncer URL points at an unreachable
/// port. Use for negative-path tests around bouncer outages.
pub fn seed_app_state_at_failing_bouncer(
    y: i32,
    m: u32,
    d: u32,
    h: u32,
    mi: u32,
    s: u32,
) -> EodRig {
    build_state(y, m, d, h, mi, s, "http://127.0.0.1:1".into())
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

pub fn ts_ms(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
    use chrono::TimeZone;
    chrono::Utc
        .with_ymd_and_hms(y, m, d, h, mi, 0)
        .unwrap()
        .timestamp_millis()
}
