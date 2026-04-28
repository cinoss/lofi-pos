mod common;

use cashier_lib::acl::Role;
use cashier_lib::app_state::{AppState, Settings};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::http::server::build_router;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::{Master, SpotKind};
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio::net::TcpListener;
use tokio_tungstenite::tungstenite::Message;

struct Rig {
    base_url: String,
    ws_url: String,
    owner_pin: String,
    staff_pin: String,
    spot_id: i64,
    client: reqwest::Client,
}

async fn boot_rig() -> Rig {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let tz = FixedOffset::east_opt(7 * 3600).unwrap();

    // Seed staff and a spot used by the lifecycle test.
    let owner_pin = "999999".to_string();
    let staff_pin = "111111".to_string();
    let spot_id = {
        let m = master.lock().unwrap();
        let owner_hash = hash_pin(&owner_pin).unwrap();
        m.create_staff("Owner", &owner_hash, Role::Owner, None)
            .unwrap();
        let staff_hash = hash_pin(&staff_pin).unwrap();
        m.create_staff("Server", &staff_hash, Role::Staff, None)
            .unwrap();
        m.create_spot("R1", SpotKind::Room, Some(50_000), None)
            .unwrap()
    };

    let event_service = EventService {
        master: master.clone(),
        events: events.clone(),
        kek: kek.clone(),
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
        kek,
        master,
        events,
        clock,
        auth,
        commands,
        store,
        settings,
        broadcast_tx,
        reports_dir: tmp.path().join("reports"),
        admin_dist: tmp.path().join("admin_dist"),
    });
    // Keep tmpdir alive for the spawned server's lifetime.
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
        ws_url: format!("ws://127.0.0.1:{port}/ws"),
        owner_pin,
        staff_pin,
        spot_id,
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

#[tokio::test]
async fn http_login_returns_token() {
    let rig = boot_rig().await;
    let resp = rig
        .client
        .post(format!("{}/auth/login", rig.base_url))
        .json(&json!({ "pin": rig.owner_pin }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    // No Set-Cookie — Bearer-only auth.
    assert!(resp.headers().get("set-cookie").is_none());
    let v: Value = resp.json().await.unwrap();
    assert!(v.get("token").is_some());
    assert!(v.get("claims").is_some());
}

#[tokio::test]
async fn http_unauthenticated_request_returns_401() {
    let rig = boot_rig().await;
    let resp = rig
        .client
        .get(format!("{}/spots", rig.base_url))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["code"], "unauthorized");
}

#[tokio::test]
async fn http_invalid_token_returns_401() {
    let rig = boot_rig().await;
    let resp = rig
        .client
        .get(format!("{}/spots", rig.base_url))
        .header("authorization", "Bearer junk")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn http_full_session_lifecycle() {
    let rig = boot_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");

    // Open session
    let session: Value = rig
        .client
        .post(format!("{}/sessions", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "idempotency_key": "open-1",
            "spot_id": rig.spot_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = session["session_id"].as_str().unwrap().to_string();
    assert_eq!(session["status"], "Open");

    // Place payment directly (orders need product seed; skip ordering for
    // this lifecycle to keep test focused on the wire path).
    let paid: Value = rig
        .client
        .post(format!("{}/sessions/{session_id}/payment", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "idempotency_key": "pay-1",
            "subtotal": 0,
            "discount_pct": 0,
            "vat_pct": 0,
            "total": 0,
            "method": "cash",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert!(paid.get("payment").is_some() || paid["status"] == "Open");

    // Close session
    let close_resp = rig
        .client
        .post(format!("{}/sessions/{session_id}/close", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({ "idempotency_key": "close-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(close_resp.status(), 200);
    let closed: Value = close_resp.json().await.unwrap();
    assert_eq!(closed["status"], "Closed");
}

#[tokio::test]
async fn http_ws_receives_event_notice_on_write() {
    let rig = boot_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");

    // Open WS connection BEFORE the write so we don't miss the broadcast.
    let (mut ws, _resp) = tokio_tungstenite::connect_async(&rig.ws_url).await.unwrap();

    // First-message handshake: send hello, expect hello_ok.
    ws.send(Message::Text(
        json!({"type": "hello", "token": token}).to_string(),
    ))
    .await
    .unwrap();
    let hello_reply = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("hello_ok not received in time")
        .expect("ws stream ended")
        .expect("ws error");
    let hello_text = match hello_reply {
        Message::Text(t) => t,
        other => panic!("unexpected ws frame: {other:?}"),
    };
    let hello_v: Value = serde_json::from_str(&hello_text).unwrap();
    assert_eq!(hello_v["type"], "hello_ok");

    // Trigger a write.
    let _: Value = rig
        .client
        .post(format!("{}/sessions", rig.base_url))
        .header("authorization", &bearer)
        .json(&json!({
            "idempotency_key": "ws-open-1",
            "spot_id": rig.spot_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();

    // Receive notice within a reasonable timeout.
    let msg = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("ws notice not received in time")
        .expect("ws stream ended")
        .expect("ws error");
    let text = match msg {
        Message::Text(t) => t,
        other => panic!("unexpected ws frame: {other:?}"),
    };
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["kind"], "event_appended");
    assert_eq!(v["event_type"], "SessionOpened");
    assert!(v["aggregate_id"].is_string());

    let _ = ws.send(Message::Close(None)).await;
}

#[tokio::test]
async fn http_ws_unauthenticated_handshake_rejected() {
    let rig = boot_rig().await;

    let (mut ws, _resp) = tokio_tungstenite::connect_async(&rig.ws_url).await.unwrap();
    ws.send(Message::Text(
        json!({"type": "hello", "token": "junk"}).to_string(),
    ))
    .await
    .unwrap();

    let reply = tokio::time::timeout(std::time::Duration::from_secs(2), ws.next())
        .await
        .expect("error frame not received in time")
        .expect("ws stream ended")
        .expect("ws error");
    let text = match reply {
        Message::Text(t) => t,
        other => panic!("unexpected ws frame: {other:?}"),
    };
    let v: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["type"], "error");
    assert_eq!(v["code"], "unauthorized");
}

#[tokio::test]
async fn http_login_rate_limit_returns_429() {
    let rig = boot_rig().await;
    let url = format!("{}/auth/login", rig.base_url);
    let bad = json!({ "pin": "000000" });
    let mut limited: Option<reqwest::Response> = None;
    for _ in 0..20 {
        let r = rig.client.post(&url).json(&bad).send().await.unwrap();
        if r.status() == 429 {
            limited = Some(r);
            break;
        }
    }
    let resp = limited.expect("expected 429 within 20 rapid attempts");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["code"], "rate_limited");
}

#[tokio::test]
async fn http_logout_revokes_token() {
    let rig = boot_rig().await;
    let token = login(&rig, &rig.owner_pin).await;
    let bearer = format!("Bearer {token}");

    // Token works pre-logout.
    let me = rig
        .client
        .get(format!("{}/auth/me", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(me.status(), 200);

    // Logout returns 204.
    let logout = rig
        .client
        .post(format!("{}/auth/logout", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(logout.status(), 204);

    // Re-using the token now returns 401.
    let again = rig
        .client
        .get(format!("{}/auth/me", rig.base_url))
        .header("authorization", &bearer)
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 401);
}

#[tokio::test]
async fn http_override_required_returns_403_with_message() {
    let rig = boot_rig().await;
    let owner_token = login(&rig, &rig.owner_pin).await;
    let owner_bearer = format!("Bearer {owner_token}");

    // Owner opens a session for the staff to attempt to close.
    let session: Value = rig
        .client
        .post(format!("{}/sessions", rig.base_url))
        .header("authorization", &owner_bearer)
        .json(&json!({
            "idempotency_key": "open-staff-1",
            "spot_id": rig.spot_id,
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let session_id = session["session_id"].as_str().unwrap().to_string();

    let staff_token = login(&rig, &rig.staff_pin).await;
    let staff_bearer = format!("Bearer {staff_token}");

    let resp = rig
        .client
        .post(format!("{}/sessions/{session_id}/close", rig.base_url))
        .header("authorization", &staff_bearer)
        .json(&json!({ "idempotency_key": "close-staff-1" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 403);
    let v: Value = resp.json().await.unwrap();
    assert_eq!(v["code"], "override_required");
    // Policy: Staff calling CloseSession needs a Cashier (or higher) override.
    assert_eq!(v["message"], "cashier");
}
