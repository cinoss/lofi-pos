mod common;

use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::error::AppError;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;
use common::room;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn rig() -> (CommandService, AuthService, Arc<EventStore>) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let mock_clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let clock: Arc<dyn Clock> = mock_clock.clone();
    let key_manager = Arc::new(cashier_lib::services::key_manager::KeyManager::new(
        master.clone(),
        kek.clone(),
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

    // Seed a Manager (PIN "999999") and a Staff (PIN "111111").
    let mh = hash_pin("999999").unwrap();
    master
        .lock()
        .unwrap()
        .create_staff("Boss", &mh, Role::Manager, None)
        .unwrap();
    let sh = hash_pin("111111").unwrap();
    master
        .lock()
        .unwrap()
        .create_staff("Worker", &sh, Role::Staff, None)
        .unwrap();

    let commands = CommandService {
        master: master.clone(),
        events: events.clone(),
        event_service,
        clock: clock.clone(),
        auth: Arc::new(auth.clone()),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
        broadcast_tx: common::dummy_broadcast(),
        store: Arc::new(AggregateStore::new()),
    };
    (commands, auth, events)
}

#[test]
fn override_pin_unblocks_action() {
    let (cs, auth, events) = rig();
    let (_, staff_claims) = auth.login("111111").unwrap();

    // Open a session as staff (Allow).
    let session_id = Uuid::new_v4().to_string();
    cs.execute(
        &staff_claims,
        Action::OpenSession,
        PolicyCtx::default(),
        "ovr-open",
        "open_session",
        &session_id,
        DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: staff_claims.staff_id,
            customer_label: None,
            team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();

    // Attempt a manager-required action without override → fails.
    let close_attempt = cs.execute(
        &staff_claims,
        Action::CloseSession,
        PolicyCtx::default(),
        "ovr-close-1",
        "close_session",
        &session_id,
        DomainEvent::SessionClosed {
            closed_by: staff_claims.staff_id,
            reason: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    );
    assert!(matches!(close_attempt, Err(AppError::OverrideRequired(_))));

    // Retry with manager override PIN → succeeds.
    let (closed, _) = cs
        .execute(
            &staff_claims,
            Action::CloseSession,
            PolicyCtx::default(),
            "ovr-close-2",
            "close_session",
            &session_id,
            DomainEvent::SessionClosed {
                closed_by: staff_claims.staff_id,
                reason: None,
            },
            Some("999999"),
            |c| c.load_session(&session_id).map(|o| o.unwrap()),
        )
        .unwrap();
    assert_eq!(
        closed.status,
        cashier_lib::domain::session::SessionStatus::Closed
    );

    // Verify the persisted event row records BOTH actor (requester) and override_staff (authorizer)
    let rows = events.list_for_aggregate(&session_id).unwrap();
    let close_row = rows
        .iter()
        .find(|r| r.event_type == "SessionClosed")
        .expect("SessionClosed event should exist");
    assert_eq!(
        close_row.actor_staff,
        Some(staff_claims.staff_id),
        "actor_staff must be the requester (Worker), not the authorizer"
    );
    assert_eq!(close_row.actor_name.as_deref(), Some("Worker"));
    assert!(
        close_row.override_staff_id.is_some(),
        "override_staff_id must be populated when override fired"
    );
    assert_ne!(
        close_row.override_staff_id,
        Some(staff_claims.staff_id),
        "override_staff_id must differ from actor_staff"
    );
    assert_eq!(close_row.override_staff_name.as_deref(), Some("Boss"));

    // Pair: the OPEN event was non-override; verify override fields are None there
    let open_row = rows
        .iter()
        .find(|r| r.event_type == "SessionOpened")
        .expect("SessionOpened event should exist");
    assert_eq!(open_row.actor_staff, Some(staff_claims.staff_id));
    assert!(
        open_row.override_staff_id.is_none(),
        "override_staff_id must be None for non-override events"
    );
    assert!(open_row.override_staff_name.is_none());
}

#[test]
fn override_pin_for_lower_role_rejected() {
    let (cs, auth, _events) = rig();
    let (_, staff_claims) = auth.login("111111").unwrap();

    let session_id = Uuid::new_v4().to_string();
    cs.execute(
        &staff_claims,
        Action::OpenSession,
        PolicyCtx::default(),
        "ovr-open-2",
        "open_session",
        &session_id,
        DomainEvent::SessionOpened {
            spot: room(1),
            opened_by: staff_claims.staff_id,
            customer_label: None,
            team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    )
    .unwrap();

    let result = cs.execute(
        &staff_claims,
        Action::CloseSession,
        PolicyCtx::default(),
        "ovr-close-bad",
        "close_session",
        &session_id,
        DomainEvent::SessionClosed {
            closed_by: staff_claims.staff_id,
            reason: None,
        },
        Some("111111"), // Staff PIN — insufficient role
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    );
    assert!(matches!(result, Err(AppError::Unauthorized)));
}
