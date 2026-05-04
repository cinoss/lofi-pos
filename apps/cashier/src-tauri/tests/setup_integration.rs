//! Integration tests for first-run setup endpoints. These use a rig that
//! deliberately does NOT seed an Owner so `needs_setup == true` and
//! `POST /admin/setup` is reachable.

mod common;

use cashier_lib::acl::Role;
use cashier_lib::app_state::{AppState, Settings};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::bouncer::client::BouncerClient;
use cashier_lib::bouncer::seed_cache::SeedCache;
use cashier_lib::http::server::build_router;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;

struct Rig {
    base_url: String,
    client: reqwest::Client,
    master: Arc<Mutex<Master>>,
}

/// Boot a rig with NO Owner and an empty venue_name (so needs_setup=true).
/// Pass `seed_owner=true` to seed an Owner + venue_name (needs_setup=false).
async fn boot_rig(seed_owner: bool) -> Rig {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds(
        "test",
        vec![("test".into(), [42u8; 32])],
    ));
    let bouncer = Arc::new(
        tokio::task::spawn_blocking(|| BouncerClient::new("http://127.0.0.1:1"))
            .await
            .unwrap(),
    );
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();

    if seed_owner {
        let m = master.lock().unwrap();
        let h = hash_pin("999999").unwrap();
        m.create_staff("Owner", &h, Role::Owner, None).unwrap();
        m.set_setting("venue_name", "Already Set").unwrap();
    }

    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(
        seed_cache.clone(),
    ));
    let event_service = EventService {
        events: events.clone(),
        key_manager: key_manager.clone(),
        clock: clock.clone(),
        cutoff_hour: 11,
        tz,
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService {
        master: master.clone(),
        clock: clock.clone(),
        signing_key: signing,
    };
    let store = Arc::new(AggregateStore::new());
    let (broadcast_tx, _rx) = tokio::sync::broadcast::channel(64);
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
    let settings = Arc::new(Settings::load(&master.lock().unwrap()).unwrap());
    let tmp = tempfile::tempdir().unwrap();
    let app_state = Arc::new(AppState {
        seed_cache,
        bouncer,
        master: master.clone(),
        events,
        key_manager,
        clock,
        auth,
        commands,
        store,
        settings,
        broadcast_tx,
        admin_dist: tmp.path().join("admin_dist"),
    });
    std::mem::forget(tmp);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let router = build_router(app_state);
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await;
    });

    Rig {
        base_url: format!("http://127.0.0.1:{port}"),
        client: reqwest::Client::new(),
        master,
    }
}

fn valid_setup_body() -> Value {
    json!({
        "venue_name": "Vua Du Karaoke",
        "venue_address": "123 Demo St",
        "venue_phone": "0900000000",
        "currency": "VND",
        "locale": "vi-VN",
        "tax_id": "",
        "receipt_footer": "Thanks!",
        "business_day_cutoff_hour": 11,
        "business_day_tz_offset_seconds": 25_200,
        "owner_name": "Boss",
        "owner_pin": "654321",
    })
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_state_returns_true_when_no_owner_or_no_venue_name() {
    let rig = boot_rig(false).await;
    let resp = rig
        .client
        .get(format!("{}/admin/setup-state", rig.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["needs_setup"], true);
    assert!(v["lan_url"].as_str().unwrap().starts_with("http://"));
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_state_returns_false_after_owner_and_venue_name_set() {
    let rig = boot_rig(true).await;
    let resp = rig
        .client
        .get(format!("{}/admin/setup-state", rig.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["needs_setup"], false);
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_post_creates_owner_and_writes_settings_atomically() {
    let rig = boot_rig(false).await;
    let resp = rig
        .client
        .post(format!("{}/admin/setup", rig.base_url))
        .json(&valid_setup_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    // Verify side effects
    let m = rig.master.lock().unwrap();
    assert_eq!(
        m.get_setting("venue_name").unwrap().as_deref(),
        Some("Vua Du Karaoke")
    );
    assert_eq!(
        m.get_setting("receipt_footer").unwrap().as_deref(),
        Some("Thanks!")
    );
    let staff = m.list_staff().unwrap();
    assert_eq!(staff.len(), 1);
    assert_eq!(staff[0].role, Role::Owner);
    assert_eq!(staff[0].name, "Boss");
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_post_seeds_room_time_product() {
    let rig = boot_rig(false).await;
    let resp = rig
        .client
        .post(format!("{}/admin/setup", rig.base_url))
        .json(&valid_setup_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 201);

    let m = rig.master.lock().unwrap();
    let products = m.list_products().unwrap();
    let time_products: Vec<_> = products.iter().filter(|p| p.kind == "time").collect();
    assert_eq!(
        time_products.len(),
        1,
        "expected exactly one kind=time product seeded by setup"
    );
    assert_eq!(time_products[0].name, "Room Time");
    assert_eq!(time_products[0].price, 0);
    assert_eq!(time_products[0].route, "none");
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_post_returns_conflict_after_setup_complete() {
    let rig = boot_rig(true).await;
    let resp = rig
        .client
        .post(format!("{}/admin/setup", rig.base_url))
        .json(&valid_setup_body())
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 409);
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_post_validates_min_pin_length() {
    let rig = boot_rig(false).await;
    let mut body = valid_setup_body();
    body["owner_pin"] = json!("123");
    let resp = rig
        .client
        .post(format!("{}/admin/setup", rig.base_url))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_post_validates_venue_name_required() {
    let rig = boot_rig(false).await;
    let mut body = valid_setup_body();
    body["venue_name"] = json!("   ");
    let resp = rig
        .client
        .post(format!("{}/admin/setup", rig.base_url))
        .json(&body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
}

#[tokio::test(flavor = "multi_thread")]
async fn setup_endpoints_do_not_require_auth() {
    // No Authorization header anywhere; endpoints must still succeed (state)
    // / be reachable (post) without one. Conflict is fine for the post path
    // when setup is already complete — what matters is we don't get 401.
    let rig = boot_rig(true).await;
    let resp = rig
        .client
        .get(format!("{}/admin/setup-state", rig.base_url))
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 401);
    let resp = rig
        .client
        .post(format!("{}/admin/setup", rig.base_url))
        .json(&valid_setup_body())
        .send()
        .await
        .unwrap();
    assert_ne!(resp.status(), 401);
}
