mod common;

use cashier_lib::bouncer::seed_cache::SeedCache;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::domain::{order, payment, session};
use cashier_lib::services::event_service::{EventService, WriteCtx};
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use chrono::FixedOffset;
use common::{item, room};
use std::sync::{Arc, Mutex};

#[test]
fn full_session_lifecycle_replays_to_expected_state() {
    let _master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds("test", vec![("test".into(), [42u8; 32])]));
    let clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 14, 0, 0));
    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(seed_cache.clone()));
    let writer = EventService {
        events: events.clone(),
        key_manager,
        clock: clock.clone(),
        cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };

    // 1. Open session
    writer
        .write(
            WriteCtx {
                aggregate_id: "sess-1",
                actor_staff: Some(7),
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &DomainEvent::SessionOpened {
                spot: room(1),
                opened_by: 7,
                customer_label: Some("VIP".into()),
                team: Some("A".into()),
            },
        )
        .unwrap();

    clock.advance_minutes(5);

    // 2. Place order
    writer
        .write(
            WriteCtx {
                aggregate_id: "ord-1",
                actor_staff: Some(7),
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &DomainEvent::OrderPlaced {
                session_id: "sess-1".into(),
                order_id: "ord-1".into(),
                items: vec![item(10, 2, 50_000), {
                    let mut it = item(11, 1, 200_000);
                    it.note = Some("ice".into());
                    it
                }],
            },
        )
        .unwrap();

    clock.advance_minutes(10);

    // 3. Cancel one item
    writer
        .write(
            WriteCtx {
                aggregate_id: "ord-1",
                actor_staff: Some(7),
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &DomainEvent::OrderItemCancelled {
                order_id: "ord-1".into(),
                item_index: 1,
                reason: Some("returned to bar".into()),
            },
        )
        .unwrap();

    clock.advance_minutes(60);

    // 4. Pay
    writer
        .write(
            WriteCtx {
                aggregate_id: "pay-1",
                actor_staff: Some(7),
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &DomainEvent::PaymentTaken {
                session_id: "sess-1".into(),
                subtotal: 100_000,
                discount_pct: 0,
                vat_pct: 8,
                total: 108_000,
                method: "cash".into(),
            },
        )
        .unwrap();

    clock.advance_minutes(1);

    // 5. Close
    writer
        .write(
            WriteCtx {
                aggregate_id: "sess-1",
                actor_staff: Some(7),
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &DomainEvent::SessionClosed {
                closed_by: 7,
                reason: None,
            },
        )
        .unwrap();

    // Verify all events for the day
    let day_rows = events.list_for_day("2026-04-27").unwrap();
    assert_eq!(day_rows.len(), 5);

    // Decrypt and project
    let session_evs: Vec<_> = events
        .list_for_aggregate("sess-1")
        .unwrap()
        .iter()
        .map(|r| writer.read_decrypted(r).unwrap())
        .collect();
    let s = session::fold("sess-1", &session_evs).unwrap();
    assert_eq!(s.status, session::SessionStatus::Closed);
    assert!(s.spot.is_room());
    assert_eq!(s.spot.id(), 1);

    let order_evs: Vec<_> = events
        .list_for_aggregate("ord-1")
        .unwrap()
        .iter()
        .map(|r| writer.read_decrypted(r).unwrap())
        .collect();
    let o = order::fold("ord-1", &order_evs).unwrap();
    assert_eq!(o.live_subtotal(), 2 * 50_000); // item 1 cancelled
    assert!(o.items[1].cancelled);

    let pay_evs: Vec<_> = events
        .list_for_aggregate("pay-1")
        .unwrap()
        .iter()
        .map(|r| writer.read_decrypted(r).unwrap())
        .collect();
    let p = payment::fold("sess-1", &pay_evs).unwrap();
    assert_eq!(p.total, 108_000);
    assert_eq!(p.method, "cash");
}

#[test]
fn seed_eviction_renders_payloads_unreadable() {
    // Crypto-shred under the bouncer model = the seed used to encrypt the
    // event no longer appears in the cashier's seed cache. With a fresh
    // KeyManager that lacks the original seed, decrypt fails.
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let original_cache = Arc::new(SeedCache::from_seeds(
        "writer",
        vec![("writer".into(), [42u8; 32])],
    ));
    let clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 14, 0, 0));
    let writer = EventService {
        events: events.clone(),
        key_manager: Arc::new(cashier_lib::services::key_manager::KeyManager::new(
            original_cache,
        )),
        clock: clock.clone(),
        cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };

    writer
        .write(
            WriteCtx {
                aggregate_id: "sess-1",
                actor_staff: None,
                actor_name: None,
                override_staff_id: None,
                override_staff_name: None,
                at: None,
            },
            &DomainEvent::SessionClosed {
                closed_by: 1,
                reason: None,
            },
        )
        .unwrap();

    let row = events.list_for_day("2026-04-27").unwrap().remove(0);
    assert!(writer.read_decrypted(&row).is_ok());

    // Simulate bouncer crypto-shred: build a new KeyManager backed by a
    // cache that does NOT contain the "writer" seed.
    let stripped_cache = Arc::new(SeedCache::from_seeds(
        "other",
        vec![("other".into(), [99u8; 32])],
    ));
    let stripped = EventService {
        events: events.clone(),
        key_manager: Arc::new(cashier_lib::services::key_manager::KeyManager::new(
            stripped_cache,
        )),
        clock,
        cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };

    // Row is still on disk (audit-trail intact), but decrypt fails.
    assert_eq!(events.list_for_day("2026-04-27").unwrap().len(), 1);
    assert!(stripped.read_decrypted(&row).is_err());
}
