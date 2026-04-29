mod common;

use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::bouncer::seed_cache::SeedCache;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::domain::session::SessionStatus;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use common::{item, table};
use std::sync::{Arc, Mutex};
use uuid::Uuid;

/// Owner-only rig (PIN "999999"). Tests that need a Staff actor add one inline.
fn rig() -> (CommandService, AuthService) {
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
    let owner_hash = hash_pin("999999").unwrap();
    master
        .lock()
        .unwrap()
        .create_staff("Owner", &owner_hash, Role::Owner, None)
        .unwrap();
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
    (commands, auth)
}

fn open_session(
    cs: &CommandService,
    claims: &cashier_lib::auth::token::TokenClaims,
    key: &str,
    table_id: Option<i64>,
) -> String {
    let session_id = Uuid::new_v4().to_string();
    let spot = table(table_id.unwrap_or(1));
    cs.execute(
        claims,
        Action::OpenSession,
        PolicyCtx::default(),
        key,
        "open_session",
        &session_id,
        DomainEvent::SessionOpened {
            spot,
            opened_by: claims.staff_id,
            customer_label: None,
            team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();
    session_id
}

#[test]
fn transfer_session_happy_path() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("999999").unwrap();
    let session_id = open_session(&cs, &claims, "ts-open", Some(1));

    let (state, _) = cs
        .execute(
            &claims,
            Action::TransferSession,
            PolicyCtx::default(),
            "ts-xfer",
            "transfer_session",
            &session_id,
            DomainEvent::SessionTransferred {
                from: table(1),
                to: table(5),
            },
            None,
            |c| c.load_session(&session_id).map(|o| o.unwrap()),
        )
        .unwrap();
    assert!(state.spot.is_table());
    assert_eq!(state.spot.id(), 5);
    assert_eq!(state.status, SessionStatus::Open);
}

#[test]
fn merge_sessions_happy_path() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("999999").unwrap();
    let target = open_session(&cs, &claims, "ms-open-t", Some(1));
    let source = open_session(&cs, &claims, "ms-open-s", Some(2));

    let (state, _) = cs
        .execute(
            &claims,
            Action::MergeSessions,
            PolicyCtx::default(),
            "ms-merge",
            "merge_sessions",
            &target,
            DomainEvent::SessionMerged {
                into_session: target.clone(),
                sources: vec![source.clone()],
            },
            None,
            |c| c.load_session(&target).map(|o| o.unwrap()),
        )
        .unwrap();
    assert_eq!(state.status, SessionStatus::Open);
    assert_eq!(state.session_id, target);
}

#[test]
fn cancel_order_item_with_manager_override() {
    let (cs, auth) = rig();
    // Add a Staff user (PIN "000000").
    let staff_hash = hash_pin("000000").unwrap();
    cs.master
        .lock()
        .unwrap()
        .create_staff("Worker", &staff_hash, Role::Staff, None)
        .unwrap();
    let (_, staff_claims) = auth.login("000000").unwrap();

    // Staff opens session + places order.
    let session_id = open_session(&cs, &staff_claims, "co-open", Some(1));
    let order_id = Uuid::new_v4().to_string();
    cs.execute(
        &staff_claims,
        Action::PlaceOrder,
        PolicyCtx::default(),
        "co-place",
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

    // Staff cancels NOT-self/NOT-grace → CancelOrderItemAny requires Manager.
    // Provide owner override PIN "999999".
    let (order, _) = cs
        .execute(
            &staff_claims,
            Action::CancelOrderItemAny,
            PolicyCtx {
                is_self: false,
                within_cancel_grace: false,
                ..PolicyCtx::default()
            },
            "co-cancel",
            "cancel_order_item",
            &order_id,
            DomainEvent::OrderItemCancelled {
                order_id: order_id.clone(),
                item_index: 0,
                reason: Some("test".into()),
            },
            Some("999999"),
            |c| c.load_order(&order_id).map(|o| o.unwrap()),
        )
        .unwrap();
    assert!(order.items[0].cancelled, "item should be marked cancelled");
}
