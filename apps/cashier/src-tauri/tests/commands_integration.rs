mod common;

use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::bouncer::seed_cache::SeedCache;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use common::{item, room};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn rig() -> (CommandService, AuthService, Arc<MockClock>) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let seed_cache = Arc::new(SeedCache::from_seeds("test", vec![("test".into(), [42u8; 32])]));
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(seed_cache.clone()));
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
    let commands = CommandService {
        master: master.clone(),
        events: events.clone(),
        event_service,
        clock: clock.clone(),
        auth: Arc::new(auth.clone()),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
        store: Arc::new(AggregateStore::new()),
        broadcast_tx: common::dummy_broadcast(),
    };

    let pin_hash = hash_pin("999999").unwrap();
    master
        .lock()
        .unwrap()
        .create_staff("Owner", &pin_hash, Role::Owner, None)
        .unwrap();
    (commands, auth, mock_clock)
}

#[test]
fn full_command_lifecycle() {
    let (cs, auth, _) = rig();
    let (_, claims) = auth.login("999999").unwrap();

    // Open session
    let session_id = Uuid::new_v4().to_string();
    let opened = cs
        .execute(
            &claims,
            Action::OpenSession,
            PolicyCtx::default(),
            "k1",
            "open_session",
            &session_id,
            DomainEvent::SessionOpened {
                spot: room(1),
                opened_by: claims.staff_id,
                customer_label: Some("VIP".into()),
                team: None,
            },
            None,
            |c| c.load_session(&session_id).map(|o| o.unwrap()),
        )
        .unwrap();
    assert!(opened.0.spot.is_room());
    assert_eq!(opened.0.spot.id(), 1);
    assert!(
        opened.0.opened_at_ms > 0,
        "opened_at_ms should be stamped from the wall-clock at write time, got {}",
        opened.0.opened_at_ms
    );

    // Place order
    let order_id = Uuid::new_v4().to_string();
    cs.execute(
        &claims,
        Action::PlaceOrder,
        PolicyCtx::default(),
        "k2",
        "place_order",
        &order_id,
        DomainEvent::OrderPlaced {
            session_id: session_id.clone(),
            order_id: order_id.clone(),
            items: vec![item(1, 2, 50_000)],
        },
        None,
        |c| c.load_order(&order_id).map(|o| o.unwrap()),
    )
    .unwrap();

    // Take payment
    cs.execute(
        &claims,
        Action::TakePayment,
        PolicyCtx::default(),
        "k3",
        "take_payment",
        &session_id,
        DomainEvent::PaymentTaken {
            session_id: session_id.clone(),
            subtotal: 100_000,
            discount_pct: 0,
            vat_pct: 8,
            total: 108_000,
            method: "cash".into(),
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();

    // Close
    let closed = cs
        .execute(
            &claims,
            Action::CloseSession,
            PolicyCtx::default(),
            "k4",
            "close_session",
            &session_id,
            DomainEvent::SessionClosed {
                closed_by: claims.staff_id,
                reason: None,
            },
            None,
            |c| c.load_session(&session_id).map(|o| o.unwrap()),
        )
        .unwrap();
    assert_eq!(
        closed.0.status,
        cashier_lib::domain::session::SessionStatus::Closed
    );
}

#[test]
fn idempotent_replay_returns_same_result_without_double_write() {
    let (cs, auth, _) = rig();
    let (_, claims) = auth.login("999999").unwrap();

    let session_id = Uuid::new_v4().to_string();
    let event = DomainEvent::SessionOpened {
        spot: room(1),
        opened_by: claims.staff_id,
        customer_label: None,
        team: None,
    };

    let first = cs
        .execute(
            &claims,
            Action::OpenSession,
            PolicyCtx::default(),
            "same-key",
            "open_session",
            &session_id,
            event.clone(),
            None,
            |c| c.load_session(&session_id).map(|o| o.unwrap()),
        )
        .unwrap();

    let second = cs
        .execute(
            &claims,
            Action::OpenSession,
            PolicyCtx::default(),
            "same-key",
            "open_session",
            &session_id,
            event,
            None,
            |c| c.load_session(&session_id).map(|o| o.unwrap()),
        )
        .unwrap();

    assert_eq!(first.0, second.0);
    assert_eq!(
        first.1,
        cashier_lib::services::command_service::WriteOutcome::Inserted
    );
    assert_eq!(
        second.1,
        cashier_lib::services::command_service::WriteOutcome::Cached
    );

    // Confirm only ONE SessionOpened event was actually written.
    let rows = cs.events.list_for_aggregate(&session_id).unwrap();
    assert_eq!(rows.len(), 1, "idempotency must prevent double-write");
}

#[test]
fn acl_denial_for_low_role_take_payment_with_large_discount() {
    let (cs, auth, _) = rig();
    // Replace owner with a Staff-role user
    let staff_pin = "111111";
    let staff_hash = cashier_lib::auth::pin::hash_pin(staff_pin).unwrap();
    cs.master
        .lock()
        .unwrap()
        .create_staff(
            "Worker",
            &staff_hash,
            cashier_lib::acl::role::Role::Staff,
            None,
        )
        .unwrap();
    let (_, claims) = auth.login(staff_pin).unwrap();

    // Open session as Staff (allowed)
    let session_id = Uuid::new_v4().to_string();
    cs.execute(
        &claims,
        Action::OpenSession,
        PolicyCtx::default(),
        "kx1",
        "open_session",
        &session_id,
        DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: claims.staff_id,
            customer_label: None,
            team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();

    // Take payment with large discount as Staff → must error with OverrideRequired(Manager)
    let result = cs.execute(
        &claims,
        Action::ApplyDiscountLarge,
        PolicyCtx {
            discount_pct: Some(50),
            discount_threshold_pct: 10,
            ..PolicyCtx::default()
        },
        "kx2",
        "take_payment",
        &session_id,
        DomainEvent::PaymentTaken {
            session_id: session_id.clone(),
            subtotal: 100,
            discount_pct: 50,
            vat_pct: 0,
            total: 50,
            method: "cash".into(),
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    );
    match result {
        Err(cashier_lib::error::AppError::OverrideRequired(role)) => {
            assert_eq!(role, cashier_lib::acl::role::Role::Manager);
        }
        other => panic!("expected OverrideRequired(Manager), got {other:?}"),
    }
}

#[test]
fn validation_failure_through_pipeline_double_close_rejected() {
    let (cs, auth, _) = rig();
    let (_, claims) = auth.login("999999").unwrap();

    let session_id = Uuid::new_v4().to_string();
    cs.execute(
        &claims,
        Action::OpenSession,
        PolicyCtx::default(),
        "ky1",
        "open_session",
        &session_id,
        DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: claims.staff_id,
            customer_label: None,
            team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();

    cs.execute(
        &claims,
        Action::CloseSession,
        PolicyCtx::default(),
        "ky2",
        "close_session",
        &session_id,
        DomainEvent::SessionClosed {
            closed_by: claims.staff_id,
            reason: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();

    // Second close — should fail validation
    let result = cs.execute(
        &claims,
        Action::CloseSession,
        PolicyCtx::default(),
        "ky3",
        "close_session",
        &session_id,
        DomainEvent::SessionClosed {
            closed_by: claims.staff_id,
            reason: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    );
    assert!(matches!(
        result,
        Err(cashier_lib::error::AppError::Conflict(_))
    ));
}
