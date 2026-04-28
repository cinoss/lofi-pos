mod common;

use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::apply::{apply, ApplyCtx};
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::day_key;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::{AppendEvent, EventStore};
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use common::{item, room};
use std::sync::{Arc, Mutex};

#[allow(clippy::too_many_arguments)]
fn write_event(
    events: &EventStore,
    master: &Master,
    kek: &Kek,
    agg: &str,
    ev: &DomainEvent,
    day: &str,
    ts: i64,
    event_type: &str,
) {
    let dek = day_key::get_or_create(master, kek, day).unwrap();
    let aad = format!("{day}|{event_type}|{agg}|{day}");
    let payload = serde_json::to_vec(ev).unwrap();
    let blob = dek.encrypt(&payload, aad.as_bytes()).unwrap();
    events
        .append(AppendEvent {
            business_day: day,
            ts,
            event_type,
            aggregate_id: agg,
            actor_staff: Some(1),
            actor_name: None,
            override_staff_id: None,
            override_staff_name: None,
            payload_enc: &blob,
            key_id: day,
        })
        .unwrap();
}

#[test]
fn warm_up_replays_live_session_with_orders() {
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let day = "2026-04-27";

    write_event(
        &events,
        &master,
        &kek,
        "sess1",
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: Some("L".into()),
            team: None,
        },
        day,
        100,
        "SessionOpened",
    );
    write_event(
        &events,
        &master,
        &kek,
        "ord1",
        &DomainEvent::OrderPlaced {
            session_id: "sess1".into(),
            order_id: "ord1".into(),
            items: vec![item(1, 2, 50)],
        },
        day,
        200,
        "OrderPlaced",
    );

    let store = AggregateStore::new();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
    let stats = store
        .warm_up(
            &master,
            &events,
            &kek,
            &clock,
            FixedOffset::east_opt(7 * 3600).unwrap(),
            11,
        )
        .unwrap();

    assert_eq!(stats.aggregates_replayed, 2);
    assert_eq!(stats.events_replayed, 2);
    let s = store.sessions.get("sess1").unwrap();
    assert_eq!(s.order_ids, vec!["ord1"]);
    assert!(store.orders.contains_key("ord1"));
}

#[test]
fn warm_up_skips_closed_sessions() {
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let day = "2026-04-27";

    write_event(
        &events,
        &master,
        &kek,
        "alive",
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
        day,
        100,
        "SessionOpened",
    );
    write_event(
        &events,
        &master,
        &kek,
        "dead",
        &DomainEvent::SessionOpened {
            spot: room(2),
            opened_by: 1,
            customer_label: None,
            team: None,
        },
        day,
        200,
        "SessionOpened",
    );
    write_event(
        &events,
        &master,
        &kek,
        "dead",
        &DomainEvent::SessionClosed {
            closed_by: 1,
            reason: None,
        },
        day,
        300,
        "SessionClosed",
    );

    let store = AggregateStore::new();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
    store
        .warm_up(
            &master,
            &events,
            &kek,
            &clock,
            FixedOffset::east_opt(7 * 3600).unwrap(),
            11,
        )
        .unwrap();

    assert!(store.sessions.contains_key("alive"));
    assert!(!store.sessions.contains_key("dead"));
}

#[test]
fn merged_session_bill_includes_source_orders() {
    // Build a full CommandService stack purely to exercise compute_bill against
    // a freshly populated AggregateStore. We bypass `execute` and call `apply`
    // directly so we can inject merge events without writing real commands.
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let event_service = EventService {
        master: master.clone(),
        events: events.clone(),
        kek: kek.clone(),
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

    // Two open sessions A and B.
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

    // One order under A (3 * 100 = 300), one under B (2 * 250 = 500).
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

    // Merge B into A: A's order_ids should now include both oA and oB.
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
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let day = "2026-04-27";

    // A is target. B is source.
    write_event(
        &events,
        &master,
        &kek,
        "A",
        &DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: 1,
            customer_label: Some("merged-target".into()),
            team: None,
        },
        day,
        100,
        "SessionOpened",
    );
    write_event(
        &events,
        &master,
        &kek,
        "B",
        &DomainEvent::SessionOpened {
            spot: room(2),
            opened_by: 1,
            customer_label: Some("absorbed".into()),
            team: None,
        },
        day,
        200,
        "SessionOpened",
    );
    // B has an order
    write_event(
        &events,
        &master,
        &kek,
        "oB",
        &DomainEvent::OrderPlaced {
            session_id: "B".into(),
            order_id: "oB".into(),
            items: vec![item(1, 1, 50_000)],
        },
        day,
        300,
        "OrderPlaced",
    );
    // Merge B into A (event written under A's aggregate)
    write_event(
        &events,
        &master,
        &kek,
        "A",
        &DomainEvent::SessionMerged {
            into_session: "A".into(),
            sources: vec!["B".into()],
        },
        day,
        400,
        "SessionMerged",
    );

    let store = AggregateStore::new();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
    store
        .warm_up(
            &master,
            &events,
            &kek,
            &clock,
            FixedOffset::east_opt(7 * 3600).unwrap(),
            11,
        )
        .unwrap();

    // Target A must be in memory, with B's order absorbed
    let a = store
        .sessions
        .get("A")
        .expect("merge target A should be in memory after warm-up");
    assert_eq!(a.order_ids, vec!["oB"], "A should have absorbed B's order");

    // Source B must NOT be in memory
    assert!(
        store.sessions.get("B").is_none(),
        "merged source B should not be in memory"
    );

    // Order oB IS in memory (it's referenced by A.order_ids)
    assert!(store.orders.contains_key("oB"));
}
