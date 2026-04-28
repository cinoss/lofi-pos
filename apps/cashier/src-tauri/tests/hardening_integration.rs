mod common;

use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::services::command_service::{CommandService, WriteOutcome};
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
use std::thread;
use uuid::Uuid;

fn rig() -> (CommandService, AuthService) {
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

    let pin = "111111";
    let h = hash_pin(pin).unwrap();
    master
        .lock()
        .unwrap()
        .create_staff("Owner", &h, Role::Owner, None)
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

#[test]
fn same_idempotency_key_under_race_yields_one_write_one_cached() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("111111").unwrap();

    let session_id = Uuid::new_v4().to_string();
    let key = "race-key-1".to_string();
    let staff_id = claims.staff_id;
    let event = DomainEvent::SessionOpened {
        spot: room(1),
        opened_by: staff_id,
        customer_label: None,
        team: None,
    };

    let cs = Arc::new(cs);
    let claims = Arc::new(claims);
    let session_id_a = Arc::new(session_id);
    let mut handles = vec![];
    for _ in 0..8 {
        let cs = cs.clone();
        let claims = claims.clone();
        let session_id = session_id_a.clone();
        let key = key.clone();
        let event = event.clone();
        handles.push(thread::spawn(move || {
            let sid = session_id.clone();
            cs.execute(
                &claims,
                Action::OpenSession,
                PolicyCtx::default(),
                &key,
                "open_session",
                &session_id,
                event,
                None,
                |c| c.load_session(&sid).map(|o| o.unwrap()),
            )
        }));
    }
    let outcomes: Vec<_> = handles
        .into_iter()
        .map(|h| h.join().unwrap().unwrap())
        .collect();
    let inserted = outcomes
        .iter()
        .filter(|(_, o)| *o == WriteOutcome::Inserted)
        .count();
    let cached = outcomes
        .iter()
        .filter(|(_, o)| *o == WriteOutcome::Cached)
        .count();
    assert_eq!(inserted, 1, "exactly one write should have occurred");
    assert_eq!(cached, 7, "seven cached responses");

    // Confirm only ONE event was actually appended.
    let rows = cs.events.list_for_aggregate(&session_id_a).unwrap();
    assert_eq!(rows.len(), 1, "race must not produce duplicate writes");
}

#[test]
fn distinct_idempotency_keys_proceed_in_parallel() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("111111").unwrap();
    let cs = Arc::new(cs);
    let claims = Arc::new(claims);
    let staff_id = claims.staff_id;

    let mut handles = vec![];
    for i in 0..8i64 {
        let cs = cs.clone();
        let claims = claims.clone();
        handles.push(thread::spawn(move || {
            let session_id = Uuid::new_v4().to_string();
            let sid = session_id.clone();
            cs.execute(
                &claims,
                Action::OpenSession,
                PolicyCtx::default(),
                &format!("k-{i}"),
                "open_session",
                &session_id,
                DomainEvent::SessionOpened {
                    spot: room(i),
                    opened_by: staff_id,
                    customer_label: None,
                    team: None,
                },
                None,
                move |c| c.load_session(&sid).map(|o| o.unwrap()),
            )
        }));
    }
    for h in handles {
        let (_, o) = h.join().unwrap().unwrap();
        assert_eq!(o, WriteOutcome::Inserted);
    }
}
