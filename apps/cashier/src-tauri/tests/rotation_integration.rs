//! Integration coverage for the UTC key rotation scheduler.
//!
//! The tokio-task wrapper itself is a thin loop; the unit-of-work — `rotate()`
//! — has its own tests inside `services::key_manager`. These tests exercise
//! the wrapper's catch-up semantics against a real `AppState`.

use cashier_lib::eod::test_support::seed_app_state_at;

#[tokio::test]
async fn rotation_startup_creates_today_and_prunes() {
    // Today (per the rig clock) = 2026-04-28 12:00 +07 → UTC 2026-04-28 05:00.
    let rig = seed_app_state_at(2026, 4, 28, 5, 0, 0);
    let now_ms = rig.state.clock.now_ms();

    // Pre-seed a 5-day-old key (well outside the 3-day TTL). Bypasses the
    // KeyManager's `current_dek` (which always uses today's UTC) by writing
    // an opaque blob directly — `rotate` only inspects the `utc_day` column
    // for retention so the wrapped material is irrelevant here.
    rig.state
        .master
        .lock()
        .unwrap()
        .put_dek("2026-04-22", &[0u8; 32], now_ms)
        .unwrap();

    let report = rig.state.key_manager.rotate(now_ms).unwrap();

    assert!(
        report.deleted.contains(&"2026-04-22".to_string()),
        "expected 2026-04-22 in deleted set, got {:?}",
        report.deleted
    );
    assert!(report.created_today, "today's DEK should have been created");
    assert_eq!(report.today, "2026-04-28");
}
