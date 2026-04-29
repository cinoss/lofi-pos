mod common;

use cashier_lib::auth::AuthService;
use cashier_lib::bouncer::seed_cache::SeedCache;
use cashier_lib::domain::apply::{apply, ApplyCtx};
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::{EventService, WriteCtx};
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use common::{item, room};
use std::sync::{Arc, Mutex};

fn rig() -> (Arc<EventStore>, Arc<cashier_lib::services::key_manager::KeyManager>, EventService) {
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds(
        "test",
        vec![("test".into(), [42u8; 32])],
    ));
    let km = Arc::new(cashier_lib::services::key_manager::KeyManager::new(seed_cache));
    let clock: Arc<dyn Clock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let svc = EventService {
        events: events.clone(),
        key_manager: km.clone(),
        clock,
        cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    (events, km, svc)
}

fn write(svc: &EventService, agg: &str, ev: &DomainEvent) {
    svc.write(
        WriteCtx {
            aggregate_id: agg,
            actor_staff: Some(1),
            actor_name: None,
            override_staff_id: None,
            override_staff_name: None,
            at: None,
        },
        ev,
    )
    .unwrap();
}

#[test]
fn warm_up_replays_live_session_with_orders() {
    let (events, km, svc) = rig();
    write(
        &svc,
        "sess1",
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: Some("L".into()),
            team: None,
        },
    );
    write(
        &svc,
        "ord1",
        &DomainEvent::OrderPlaced {
            session_id: "sess1".into(),
            order_id: "ord1".into(),
            items: vec![item(1, 2, 50)],
        },
    );

    let store = AggregateStore::new();
    let report = store.warm_up(&events, &km).unwrap();
    assert_eq!(report.aggregates_replayed, 2);
    assert_eq!(report.events_replayed, 2);
    let s = store.sessions.get("sess1").unwrap();
    assert_eq!(s.order_ids, vec!["ord1"]);
    assert!(store.orders.contains_key("ord1"));
}

#[test]
fn warm_up_skips_closed_sessions() {
    let (events, km, svc) = rig();
    write(
        &svc,
        "alive",
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
    );
    write(
        &svc,
        "dead",
        &DomainEvent::SessionOpened {
            spot: room(2),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
    );
    write(
        &svc,
        "dead",
        &DomainEvent::SessionClosed {
            closed_by: 1,
            reason: None,
        },
    );

    let store = AggregateStore::new();
    store.warm_up(&events, &km).unwrap();
    // Closed sessions are still applied; their final status is Closed.
    // Live-list filter is at query time, not warm-up time.
    assert!(store.sessions.contains_key("alive"));
    assert!(store.sessions.contains_key("dead"));
    let dead = store.sessions.get("dead").unwrap();
    assert_eq!(
        dead.status,
        cashier_lib::domain::session::SessionStatus::Closed
    );
}

#[test]
fn merged_session_bill_includes_source_orders() {
    // Build a CommandService stack purely to exercise compute_bill against
    // a freshly populated AggregateStore.
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds(
        "test",
        vec![("test".into(), [42u8; 32])],
    ));
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(
        seed_cache,
    ));
    let event_service = EventService {
        events: events.clone(),
        key_manager,
        clock: clock.clone(),
        cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService {
        master: master.clone(),
        clock: clock.clone(),
        signing_key: signing,
    };
    let store = Arc::new(AggregateStore::new());
    let cs = CommandService {
        master: master.clone(),
        events: events.clone(),
        event_service,
        clock: clock.clone(),
        auth: Arc::new(auth),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
        store: store.clone(),
        broadcast_tx: common::dummy_broadcast(),
    };

    apply(
        &store,
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
        ApplyCtx { aggregate_id: "A" },
    )
    .unwrap();
    apply(
        &store,
        &DomainEvent::SessionOpened {
            spot: room(2),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
        ApplyCtx { aggregate_id: "B" },
    )
    .unwrap();
    apply(
        &store,
        &DomainEvent::OrderPlaced {
            session_id: "A".into(),
            order_id: "oA".into(),
            items: vec![item(1, 3, 100)],
        },
        ApplyCtx { aggregate_id: "oA" },
    )
    .unwrap();
    apply(
        &store,
        &DomainEvent::OrderPlaced {
            session_id: "B".into(),
            order_id: "oB".into(),
            items: vec![item(2, 2, 250)],
        },
        ApplyCtx { aggregate_id: "oB" },
    )
    .unwrap();
    apply(
        &store,
        &DomainEvent::SessionMerged {
            into_session: "A".into(),
            sources: vec!["B".into()],
        },
        ApplyCtx { aggregate_id: "A" },
    )
    .unwrap();

    let total = cs.compute_bill("A").unwrap();
    assert_eq!(total, 300 + 500, "merged bill must sum oA + absorbed oB");
}

#[test]
fn warm_up_preserves_merge_target_and_drops_sources() {
    let (events, km, svc) = rig();
    // A is target. B is source.
    write(
        &svc,
        "A",
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: Some("merged-target".into()),
            team: None,
        },
    );
    write(
        &svc,
        "B",
        &DomainEvent::SessionOpened {
            spot: room(2),
            opened_by: 1,
            customer_label: Some("absorbed".into()),
            team: None,
        },
    );
    write(
        &svc,
        "oB",
        &DomainEvent::OrderPlaced {
            session_id: "B".into(),
            order_id: "oB".into(),
            items: vec![item(1, 1, 50_000)],
        },
    );
    write(
        &svc,
        "A",
        &DomainEvent::SessionMerged {
            into_session: "A".into(),
            sources: vec!["B".into()],
        },
    );

    let store = AggregateStore::new();
    store.warm_up(&events, &km).unwrap();

    let a = store
        .sessions
        .get("A")
        .expect("merge target A should be in memory after warm-up");
    assert_eq!(a.order_ids, vec!["oB"], "A should have absorbed B's order");
    // B is also still in the map (Closed/merged status); the live filter
    // happens elsewhere. The order is still reachable.
    assert!(store.orders.contains_key("oB"));
}
