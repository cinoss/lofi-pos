use cashier_lib::store::migrations::{run_migrations, MASTER_MIGRATIONS};
use rusqlite::Connection;

#[test]
fn applies_migrations_to_fresh_db() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, &MASTER_MIGRATIONS).unwrap();
    let count: i64 = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
        .unwrap();
    assert!(count >= 1, "expected at least one migration applied");
}

#[test]
fn migrations_are_idempotent() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, &MASTER_MIGRATIONS).unwrap();
    let first: i64 = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
        .unwrap();
    run_migrations(&mut conn, &MASTER_MIGRATIONS).unwrap();
    let second: i64 = conn
        .query_row("SELECT COUNT(*) FROM _migrations", [], |r| r.get(0))
        .unwrap();
    assert_eq!(first, second, "second run should not re-apply");
}

#[test]
fn expected_tables_exist_after_migration() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, &MASTER_MIGRATIONS).unwrap();
    let tables = [
        "staff",
        "spot",
        "product",
        "recipe",
        "setting",
        "dek",
        "daily_report",
        "idempotency_key",
        "token_denylist",
        "_migrations",
    ];
    for t in tables {
        let exists: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
                rusqlite::params![t],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(exists, 1, "missing table: {t}");
    }
}

#[test]
fn default_settings_seeded() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn, &MASTER_MIGRATIONS).unwrap();
    let val: String = conn
        .query_row(
            "SELECT value FROM setting WHERE key='business_day_cutoff_hour'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(val, "11");
}
