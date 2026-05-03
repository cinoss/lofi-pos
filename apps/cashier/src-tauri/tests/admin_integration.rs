//! Plan F: integration tests for /admin/* CRUD + /admin/reports/* endpoints.
//! Boots a fresh axum server per test, drives it via reqwest.

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
    owner_pin: String,
    cashier_pin: String,
    client: reqwest::Client,
}

async fn boot_admin_rig() -> Rig {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds("test", vec![("test".into(), [42u8; 32])]));
    // BouncerClient eagerly builds a reqwest::blocking client; construct it
    // off-runtime so the inner tokio runtime can be dropped cleanly later.
    let bouncer = Arc::new(
        tokio::task::spawn_blocking(|| BouncerClient::new("http://127.0.0.1:1"))
            .await
            .unwrap(),
    );
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();

    let owner_pin = "999999".to_string();
    let cashier_pin = "111111".to_string();
    {
        let m = master.lock().unwrap();
        let owner_hash = hash_pin(&owner_pin).unwrap();
        m.create_staff("Owner", &owner_hash, Role::Owner, None)
            .unwrap();
        let cashier_hash = hash_pin(&cashier_pin).unwrap();
        m.create_staff("Cashier", &cashier_hash, Role::Cashier, None)
            .unwrap();
    }

    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(seed_cache.clone()));
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
        master,
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
        owner_pin,
        cashier_pin,
        client: reqwest::Client::new(),
    }
}

async fn login(rig: &Rig, pin: &str) -> String {
    let resp = rig
        .client
        .post(format!("{}/auth/login", rig.base_url))
        .json(&json!({ "pin": pin }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: Value = resp.json().await.unwrap();
    v["token"].as_str().unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_spot_crud_owner_can_create_update_delete() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");

    // CREATE
    let create: Value = rig
        .client
        .post(format!("{}/admin/spots", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "name": "VIP-1",
            "kind": "room",
            "billing_config": {
                "hourly_rate": 100_000,
                "bucket_minutes": 1,
                "included_minutes": 0,
                "min_charge": 0,
            },
            "parent_id": null,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = create["id"].as_i64().unwrap();
    assert_eq!(create["name"], "VIP-1");
    assert_eq!(create["kind"], "room");

    // LIST
    let list: Value = rig
        .client
        .get(format!("{}/admin/spots", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(list.as_array().unwrap().len(), 1);

    // UPDATE
    let updated: Value = rig
        .client
        .put(format!("{}/admin/spots/{id}", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "name": "VIP-1-renamed",
            "kind": "room",
            "billing_config": {
                "hourly_rate": 200_000,
                "bucket_minutes": 1,
                "included_minutes": 0,
                "min_charge": 0,
            },
            "parent_id": null,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["name"], "VIP-1-renamed");
    assert_eq!(updated["billing_config"]["hourly_rate"], 200_000);

    // DELETE
    let resp = rig
        .client
        .delete(format!("{}/admin/spots/{id}", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_spot_create_forbidden_for_cashier() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.cashier_pin).await;
    let resp = rig
        .client
        .post(format!("{}/admin/spots", rig.base_url))
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({
            "name": "X", "kind": "room",
            "billing_config": {"hourly_rate": 1, "bucket_minutes": 1, "included_minutes": 0, "min_charge": 0},
            "parent_id": null,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_staff_crud_owner_can_create_and_list() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");
    let created: Value = rig
        .client
        .post(format!("{}/admin/staff", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "name": "Bob",
            "pin": "654321",
            "role": "manager",
            "team": "A",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["role"], "manager");

    // List
    let list: Value = rig
        .client
        .get(format!("{}/admin/staff", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    // Owner + Cashier (seeded) + Bob
    assert_eq!(list.as_array().unwrap().len(), 3);

    // Delete
    let resp = rig
        .client
        .delete(format!("{}/admin/staff/{id}", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_staff_create_forbidden_for_cashier() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.cashier_pin).await;
    let resp = rig
        .client
        .post(format!("{}/admin/staff", rig.base_url))
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({
            "name": "X", "pin": "abcdef", "role": "staff", "team": null,
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_product_crud_owner_can_create_update_delete() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");
    let created: Value = rig
        .client
        .post(format!("{}/admin/products", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "name": "Beer", "price": 50_000, "route": "bar", "kind": "item",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["name"], "Beer");

    let updated: Value = rig
        .client
        .put(format!("{}/admin/products/{id}", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "name": "Beer XL", "price": 60_000, "route": "bar", "kind": "item",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(updated["price"], 60_000);

    let resp = rig
        .client
        .delete(format!("{}/admin/products/{id}", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 204);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_product_create_forbidden_for_cashier() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.cashier_pin).await;
    let resp = rig
        .client
        .post(format!("{}/admin/products", rig.base_url))
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({
            "name": "X", "price": 1, "route": "bar", "kind": "item",
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_settings_get_and_update() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");

    let before: Value = rig
        .client
        .get(format!("{}/admin/settings", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(before["business_day_cutoff_hour"], 11);

    let after: Value = rig
        .client
        .put(format!("{}/admin/settings", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "business_day_cutoff_hour": 4,
            "discount_threshold_pct": 20,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(after["business_day_cutoff_hour"], 4);
    assert_eq!(after["discount_threshold_pct"], 20);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_settings_update_forbidden_for_cashier() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.cashier_pin).await;
    let resp = rig
        .client
        .put(format!("{}/admin/settings", rig.base_url))
        .header("authorization", format!("Bearer {token}"))
        .json(&json!({"business_day_cutoff_hour": 4}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test(flavor = "multi_thread")]
async fn ui_admin_serves_index_html_with_spa_fallback() {
    // Build a fresh rig and seed an admin_dist with a stub index.html, so
    // we can prove ServeDir is mounted and the SPA fallback fires for
    // unknown paths.
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds("test", vec![("test".into(), [42u8; 32])]));
    let bouncer = Arc::new(
        tokio::task::spawn_blocking(|| BouncerClient::new("http://127.0.0.1:1"))
            .await
            .unwrap(),
    );
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();
    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(seed_cache.clone()));
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
    let admin_dist = tmp.path().join("admin");
    std::fs::create_dir_all(&admin_dist).unwrap();
    std::fs::write(admin_dist.join("index.html"), "<html>hi from admin</html>").unwrap();
    let app_state = Arc::new(AppState {
        seed_cache,
        bouncer,
        master,
        events,
        key_manager,
        clock,
        auth,
        commands,
        store,
        settings,
        broadcast_tx,
        admin_dist: admin_dist.clone(),
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
    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();

    // Direct index hit (explicit filename — ServeDir always serves that).
    let resp = client
        .get(format!("{base}/ui/admin/index.html"))
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();
    eprintln!("status={status} body={body:?} admin_dist={admin_dist:?} exists={}", admin_dist.join("index.html").exists());
    assert_eq!(status, 200);
    assert!(body.contains("hi from admin"));

    // SPA fallback for an unknown route → returns index.html via not_found_service.
    let resp = client
        .get(format!("{base}/ui/admin/spots"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("hi from admin"));
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_staff_update_team_null_clears_team_absent_leaves_it() {
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");

    // Create staff with team=A.
    let created: Value = rig
        .client
        .post(format!("{}/admin/staff", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "name": "Alice",
            "pin": "234567",
            "role": "staff",
            "team": "A",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let id = created["id"].as_i64().unwrap();
    assert_eq!(created["team"], "A");

    // PUT without `team` field — must leave team alone (still "A").
    let resp = rig
        .client
        .put(format!("{}/admin/staff/{id}", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({ "name": "Alice2" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["name"], "Alice2");
    assert_eq!(body["team"], "A", "absent team field must not clear team");

    // PUT with `team: null` — must clear team.
    let resp = rig
        .client
        .put(format!("{}/admin/staff/{id}", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({ "team": null }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert!(body["team"].is_null(), "team:null must clear (got {:?})", body["team"]);

    // PUT with `team: "B"` — must set to B.
    let resp = rig
        .client
        .put(format!("{}/admin/staff/{id}", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({ "team": "B" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["team"], "B");
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_reports_route_is_gone() {
    // Reports moved off-box to the bouncer; the cashier no longer mounts the
    // /admin/reports route at all.
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let resp = rig
        .client
        .get(format!("{}/admin/reports", rig.base_url))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}

#[tokio::test(flavor = "multi_thread")]
async fn admin_keys_route_is_gone() {
    // Keys live in the bouncer; cashier no longer exposes them.
    let rig = boot_admin_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let resp = rig
        .client
        .get(format!("{}/admin/keys", rig.base_url))
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 404);
}
