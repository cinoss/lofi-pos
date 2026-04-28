# Foundation Plan B — Event Store + Domain + Projections

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the event-sourced transactional layer: `events.db` with column-encrypted payloads, per-day DEK lifecycle, business-day calculation, domain aggregates (Session, Order, Payment), and projection folds. End state: a domain service can open a session, append events, replay them into projections, all with crypto-shred-ready storage.

**Architecture:** Separate `events.db` file (per spec, distinct from `master.db`). One append-only `event` table; payloads encrypted per business day with random DEK wrapped under KEK. Plaintext indexed columns (`business_day`, `aggregate_id`, `type`, `ts`, `key_id`). Domain layer = pure functions over event sequences yielding immutable projection structs. All time goes through an injectable `Clock`.

**Tech Stack:** Adds `chrono = "0.4"` (TZ-aware business-day math) to existing Rust deps. No new TS deps.

**Spec:** `docs/superpowers/specs/2026-04-27-foundation-design.md`
**Builds on:** Plan A (`apps/cashier/src-tauri`) — KEK/DEK crypto, master DB, migration runner

---

## File Structure

```
apps/cashier/src-tauri/src/
  error.rs                              # MODIFIED — add Conflict, Unauthorized variants
  time.rs                               # NEW — Clock trait + SystemClock + MockClock (test)
  business_day.rs                       # NEW — cutoff math
  store/
    master.rs                           # MODIFIED — add with_tx<F, R>
    events.rs                           # NEW — EventStore: open, append, list_for_day, list_for_aggregate, delete_day
    migrations/
      0002_events.sql                   # NEW — event table + indexes (in events.db, not master.db)
  domain/
    mod.rs                              # NEW
    event.rs                            # NEW — DomainEvent enum (SessionOpened, OrderPlaced, ...) + Aggregate enum
    session.rs                          # NEW — SessionState projection + fold
    order.rs                            # NEW — OrderState projection + fold
    payment.rs                          # NEW — PaymentState projection + fold
  services/
    mod.rs                              # NEW
    day_key.rs                          # NEW — get_or_create_dek_for(business_day) using Master + Kek
    event_writer.rs                     # NEW — write_event(envelope) — high-level append wrapping crypto + tx
  testkit.rs                            # NEW (cfg(test) only) — builders, MockClock factory, temp DBs
tests/
  events_integration.rs                 # NEW — append + read + delete_day
  day_key_lifecycle.rs                  # NEW — first event creates DEK row; subsequent reuse; delete shreds
  projection_integration.rs             # NEW — full session lifecycle replay
```

Plan A's `events.db` is opened as a SEPARATE file at `<data_dir>/events.db`. The migration runner is reused but pointed at a new directory of SQL files (or we add a runner mode that takes a directory; cleaner: extract `run_migrations_from(&Dir, &mut Connection)` and have two callers).

---

## Task 1: Add `AppError::Conflict` and `AppError::Unauthorized`

**Files:** Modify `apps/cashier/src-tauri/src/error.rs`

- [ ] **Step 1: Edit `error.rs`**

Add two variants to the `AppError` enum:
```rust
#[error("conflict: {0}")] Conflict(String),
#[error("unauthorized")] Unauthorized,
```
Final enum should be (in order): `Db`, `Crypto`, `Keychain`, `Io`, `NotFound`, `Validation`, `Config`, `Conflict`, `Unauthorized`.

- [ ] **Step 2: Verify compiles**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: clean.

- [ ] **Step 3: Commit**

```bash
git add apps/cashier/src-tauri/src/error.rs
git commit -m "feat(cashier): add Conflict and Unauthorized to AppError"
```

---

## Task 2: `Clock` trait

**Files:**
- Create: `apps/cashier/src-tauri/src/time.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (add `pub mod time;`)

- [ ] **Step 1: Add `chrono` dependency**

Edit `apps/cashier/src-tauri/Cargo.toml` `[dependencies]`:
```toml
chrono = { version = "0.4", default-features = false, features = ["clock", "std"] }
```

- [ ] **Step 2: Write the failing test**

Create `apps/cashier/src-tauri/src/time.rs`:
```rust
use chrono::{DateTime, Utc};

pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
    fn now_ms(&self) -> i64 {
        self.now().timestamp_millis()
    }
}

pub struct SystemClock;
impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> { Utc::now() }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use chrono::TimeZone;
    use std::sync::Mutex;

    pub struct MockClock(Mutex<DateTime<Utc>>);

    impl MockClock {
        pub fn new(t: DateTime<Utc>) -> Self { Self(Mutex::new(t)) }
        pub fn at_ymd_hms(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> Self {
            Self::new(Utc.with_ymd_and_hms(y, m, d, h, mi, s).unwrap())
        }
        pub fn set(&self, t: DateTime<Utc>) { *self.0.lock().unwrap() = t; }
        pub fn advance_minutes(&self, n: i64) {
            let mut g = self.0.lock().unwrap();
            *g = *g + chrono::Duration::minutes(n);
        }
    }
    impl Clock for MockClock {
        fn now(&self) -> DateTime<Utc> { *self.0.lock().unwrap() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::MockClock;

    #[test]
    fn system_clock_now_ms_increases() {
        let c = SystemClock;
        let a = c.now_ms();
        std::thread::sleep(std::time::Duration::from_millis(2));
        let b = c.now_ms();
        assert!(b > a);
    }

    #[test]
    fn mock_clock_returns_set_time() {
        let c = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
        assert_eq!(c.now().timestamp(), 1777_896_000);
    }

    #[test]
    fn mock_clock_advance() {
        let c = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
        c.advance_minutes(30);
        assert_eq!(c.now().timestamp(), 1777_897_800);
    }
}
```

- [ ] **Step 3: Wire into lib**

Modify `apps/cashier/src-tauri/src/lib.rs`. Add after `pub mod bootstrap;`:
```rust
pub mod time;
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib time`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/cashier/src-tauri/src/time.rs apps/cashier/src-tauri/src/lib.rs apps/cashier/src-tauri/Cargo.toml apps/cashier/src-tauri/Cargo.lock
git commit -m "feat(cashier): Clock trait with SystemClock + MockClock test fake"
```

---

## Task 3: Business-day math

**Files:**
- Create: `apps/cashier/src-tauri/src/business_day.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (add `pub mod business_day;`)

- [ ] **Step 1: Write tests + impl**

Create `apps/cashier/src-tauri/src/business_day.rs`:
```rust
use chrono::{DateTime, Datelike, Duration, Utc};

/// A business day spans `cutoff_hour` (local time, but we use UTC for simplicity)
/// to `cutoff_hour` next day. Default cutoff = 11:00.
/// An event at time `t` belongs to business day = (t - cutoff_hour hours).date.
pub fn business_day_of(t: DateTime<Utc>, cutoff_hour: u32) -> String {
    let shifted = t - Duration::hours(cutoff_hour as i64);
    format!("{:04}-{:02}-{:02}", shifted.year(), shifted.month(), shifted.day())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn dt(y: i32, m: u32, d: u32, h: u32, mi: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap()
    }

    #[test]
    fn before_cutoff_belongs_to_previous_day() {
        // 2026-04-28 03:00 with cutoff 11 → previous day (2026-04-27)
        assert_eq!(business_day_of(dt(2026, 4, 28, 3, 0), 11), "2026-04-27");
    }

    #[test]
    fn at_cutoff_belongs_to_new_day() {
        // 2026-04-28 11:00 with cutoff 11 → 2026-04-28 (boundary inclusive)
        assert_eq!(business_day_of(dt(2026, 4, 28, 11, 0), 11), "2026-04-28");
    }

    #[test]
    fn after_cutoff_belongs_to_new_day() {
        assert_eq!(business_day_of(dt(2026, 4, 28, 12, 0), 11), "2026-04-28");
    }

    #[test]
    fn midnight_with_cutoff_11_is_previous_day() {
        assert_eq!(business_day_of(dt(2026, 4, 28, 0, 0), 11), "2026-04-27");
    }

    #[test]
    fn midnight_with_cutoff_0_is_same_day() {
        assert_eq!(business_day_of(dt(2026, 4, 28, 0, 0), 0), "2026-04-28");
    }

    #[test]
    fn cutoff_22_late_night() {
        // 2026-04-28 21:00 with cutoff 22 → still 2026-04-27
        assert_eq!(business_day_of(dt(2026, 4, 28, 21, 0), 22), "2026-04-27");
        assert_eq!(business_day_of(dt(2026, 4, 28, 22, 0), 22), "2026-04-28");
    }
}
```

- [ ] **Step 2: Wire into lib**

Modify `apps/cashier/src-tauri/src/lib.rs`. Add:
```rust
pub mod business_day;
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib business_day`
Expected: 6 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/business_day.rs apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): business_day_of helper with cutoff hour"
```

---

## Task 4: Refactor migration runner to take a directory

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/migrations.rs`

- [ ] **Step 1: Refactor**

Replace `apps/cashier/src-tauri/src/store/migrations.rs`:
```rust
use crate::error::{AppError, AppResult};
use include_dir::{include_dir, Dir};
use rusqlite::{params, Connection};

pub static MASTER_MIGRATIONS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations/master");
pub static EVENTS_MIGRATIONS: Dir<'_> =
    include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations/events");

pub fn run_migrations(conn: &mut Connection, dir: &Dir<'static>) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
             name TEXT PRIMARY KEY,
             applied_at INTEGER NOT NULL
         )",
    )?;

    let mut files: Vec<_> = dir
        .files()
        .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    files.sort_by_key(|f| f.path().to_owned());

    for file in files {
        let name = file
            .path()
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| AppError::Validation("bad migration filename".into()))?
            .to_string();
        let applied: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM _migrations WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .ok();
        if applied.is_some() {
            continue;
        }

        let sql = file
            .contents_utf8()
            .ok_or_else(|| AppError::Validation(format!("non-utf8 migration {name}")))?;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO _migrations(name, applied_at) VALUES (?1, ?2)",
            params![name, now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 2: Move existing master migration**

Run:
```bash
mkdir -p apps/cashier/src-tauri/src/store/migrations/master
mkdir -p apps/cashier/src-tauri/src/store/migrations/events
git mv apps/cashier/src-tauri/src/store/migrations/0001_init.sql \
       apps/cashier/src-tauri/src/store/migrations/master/0001_init.sql
git mv apps/cashier/src-tauri/src/store/migrations/.gitkeep \
       apps/cashier/src-tauri/src/store/migrations/master/.gitkeep
touch apps/cashier/src-tauri/src/store/migrations/events/.gitkeep
```

- [ ] **Step 3: Update `Master::open*` callers**

Edit `apps/cashier/src-tauri/src/store/master.rs`. Find the two call sites:
```rust
crate::store::migrations::run_migrations(&mut conn)?;
```
Change BOTH to:
```rust
crate::store::migrations::run_migrations(&mut conn, &crate::store::migrations::MASTER_MIGRATIONS)?;
```

- [ ] **Step 4: Update `migrations_integration.rs` test calls**

Edit `apps/cashier/src-tauri/tests/migrations_integration.rs`. The 4 tests call `run_migrations(&mut conn)` — change all to:
```rust
use cashier_lib::store::migrations::{run_migrations, MASTER_MIGRATIONS};
// ...
run_migrations(&mut conn, &MASTER_MIGRATIONS).unwrap();
```

- [ ] **Step 5: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all 24 prior tests still pass (3 new from time, 6 new from business_day = 33 total now).

- [ ] **Step 6: Commit**

```bash
git add -u apps/cashier/src-tauri/src apps/cashier/src-tauri/tests
git add apps/cashier/src-tauri/src/store/migrations/events/.gitkeep
git commit -m "refactor(cashier): split migrations into master and events directories"
```

---

## Task 5: Add `Master::with_tx`

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: Write failing test**

Append to `apps/cashier/src-tauri/src/store/master.rs` `mod tests`:
```rust
    #[test]
    fn with_tx_commits_on_ok() {
        let mut m = Master::open_in_memory().unwrap();
        let r = m.with_tx(|tx| {
            tx.execute(
                "INSERT INTO setting(key, value) VALUES('tx_test', 'a')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )?;
            Ok(())
        });
        assert!(r.is_ok());
        assert_eq!(m.get_setting("tx_test").unwrap().as_deref(), Some("a"));
    }

    #[test]
    fn with_tx_rolls_back_on_err() {
        let mut m = Master::open_in_memory().unwrap();
        let r: AppResult<()> = m.with_tx(|tx| {
            tx.execute(
                "INSERT INTO setting(key, value) VALUES('tx_rollback', 'x')
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [],
            )?;
            Err(crate::error::AppError::Validation("force".into()))
        });
        assert!(r.is_err());
        assert!(m.get_setting("tx_rollback").unwrap().is_none());
    }
```

- [ ] **Step 2: Implement `with_tx`**

Add inside `impl Master`:
```rust
    pub fn with_tx<F, R>(&mut self, f: F) -> AppResult<R>
    where
        F: FnOnce(&rusqlite::Transaction<'_>) -> AppResult<R>,
    {
        let tx = self.conn.transaction()?;
        let r = f(&tx)?;
        tx.commit()?;
        Ok(r)
    }
```

Note: `&mut self` because `Connection::transaction` requires it.

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib store::master`
Expected: 6 tests pass (4 prior + 2 new).

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/store/master.rs
git commit -m "feat(cashier): Master::with_tx for caller-driven transactions"
```

---

## Task 6: `events.db` migration + `EventStore` skeleton

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/events/0001_events.sql`
- Create: `apps/cashier/src-tauri/src/store/events.rs`
- Modify: `apps/cashier/src-tauri/src/store/mod.rs`

- [ ] **Step 1: Create events schema**

Create `apps/cashier/src-tauri/src/store/migrations/events/0001_events.sql`:
```sql
CREATE TABLE event (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  business_day  TEXT NOT NULL,
  ts            INTEGER NOT NULL,
  type          TEXT NOT NULL,
  aggregate_id  TEXT NOT NULL,
  actor_staff   INTEGER,
  payload_enc   BLOB NOT NULL,
  key_id        TEXT NOT NULL
);

CREATE INDEX idx_event_day ON event(business_day);
CREATE INDEX idx_event_agg ON event(aggregate_id, id);
CREATE INDEX idx_event_day_type ON event(business_day, type);
```

- [ ] **Step 2: Create `EventStore`**

Create `apps/cashier/src-tauri/src/store/events.rs`:
```rust
use crate::error::AppResult;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;

pub struct EventStore {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct EventRow {
    pub id: i64,
    pub business_day: String,
    pub ts: i64,
    pub event_type: String,
    pub aggregate_id: String,
    pub actor_staff: Option<i64>,
    pub payload_enc: Vec<u8>,
    pub key_id: String,
}

#[derive(Debug, Clone)]
pub struct AppendEvent<'a> {
    pub business_day: &'a str,
    pub ts: i64,
    pub event_type: &'a str,
    pub aggregate_id: &'a str,
    pub actor_staff: Option<i64>,
    pub payload_enc: &'a [u8],
    pub key_id: &'a str,
}

impl EventStore {
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(
            &mut conn,
            &crate::store::migrations::EVENTS_MIGRATIONS,
        )?;
        Ok(Self { conn })
    }
    pub fn open_in_memory() -> AppResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(
            &mut conn,
            &crate::store::migrations::EVENTS_MIGRATIONS,
        )?;
        Ok(Self { conn })
    }

    pub fn append(&self, ev: AppendEvent<'_>) -> AppResult<i64> {
        self.conn.execute(
            "INSERT INTO event
             (business_day, ts, type, aggregate_id, actor_staff, payload_enc, key_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                ev.business_day, ev.ts, ev.event_type, ev.aggregate_id,
                ev.actor_staff, ev.payload_enc, ev.key_id
            ],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn list_for_day(&self, business_day: &str) -> AppResult<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, business_day, ts, type, aggregate_id, actor_staff, payload_enc, key_id
             FROM event WHERE business_day = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![business_day], row_to_event)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn list_for_aggregate(&self, aggregate_id: &str) -> AppResult<Vec<EventRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, business_day, ts, type, aggregate_id, actor_staff, payload_enc, key_id
             FROM event WHERE aggregate_id = ?1 ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map(params![aggregate_id], row_to_event)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn count_for_day(&self, business_day: &str) -> AppResult<i64> {
        Ok(self.conn.query_row(
            "SELECT COUNT(*) FROM event WHERE business_day = ?1",
            params![business_day],
            |r| r.get(0),
        )?)
    }

    pub fn delete_day(&self, business_day: &str) -> AppResult<usize> {
        let n = self.conn.execute(
            "DELETE FROM event WHERE business_day = ?1",
            params![business_day],
        )?;
        Ok(n)
    }

    pub fn vacuum(&self) -> AppResult<()> {
        self.conn.execute_batch("VACUUM")?;
        Ok(())
    }

    /// Look up the most recent event for an aggregate, or None.
    pub fn latest_for_aggregate(&self, aggregate_id: &str) -> AppResult<Option<EventRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, business_day, ts, type, aggregate_id, actor_staff, payload_enc, key_id
                 FROM event WHERE aggregate_id = ?1 ORDER BY id DESC LIMIT 1",
                params![aggregate_id],
                row_to_event,
            )
            .optional()?)
    }
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        id: r.get(0)?,
        business_day: r.get(1)?,
        ts: r.get(2)?,
        event_type: r.get(3)?,
        aggregate_id: r.get(4)?,
        actor_staff: r.get(5)?,
        payload_enc: r.get(6)?,
        key_id: r.get(7)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(day: &str, agg: &str, ty: &str, ts: i64) -> AppendEvent<'static> {
        // Use leaked strings just for the test ergonomics.
        AppendEvent {
            business_day: Box::leak(day.to_string().into_boxed_str()),
            ts,
            event_type: Box::leak(ty.to_string().into_boxed_str()),
            aggregate_id: Box::leak(agg.to_string().into_boxed_str()),
            actor_staff: Some(1),
            payload_enc: Box::leak(b"ciphertext".to_vec().into_boxed_slice()),
            key_id: Box::leak(day.to_string().into_boxed_str()),
        }
    }

    #[test]
    fn append_then_list_for_day() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "sess-1", "SessionOpened", 100)).unwrap();
        s.append(ev("2026-04-27", "sess-1", "OrderPlaced", 200)).unwrap();
        let rows = s.list_for_day("2026-04-27").unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].event_type, "SessionOpened");
        assert_eq!(rows[1].event_type, "OrderPlaced");
        assert!(rows[0].id < rows[1].id);
    }

    #[test]
    fn list_for_aggregate_filters() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "sess-1", "SessionOpened", 100)).unwrap();
        s.append(ev("2026-04-27", "sess-2", "SessionOpened", 110)).unwrap();
        let rows = s.list_for_aggregate("sess-1").unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].aggregate_id, "sess-1");
    }

    #[test]
    fn delete_day_removes_only_that_day() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "a", "X", 1)).unwrap();
        s.append(ev("2026-04-28", "b", "Y", 2)).unwrap();
        let n = s.delete_day("2026-04-27").unwrap();
        assert_eq!(n, 1);
        assert_eq!(s.count_for_day("2026-04-27").unwrap(), 0);
        assert_eq!(s.count_for_day("2026-04-28").unwrap(), 1);
    }

    #[test]
    fn latest_for_aggregate_returns_newest() {
        let s = EventStore::open_in_memory().unwrap();
        s.append(ev("2026-04-27", "a", "X", 1)).unwrap();
        s.append(ev("2026-04-27", "a", "Y", 2)).unwrap();
        let last = s.latest_for_aggregate("a").unwrap().unwrap();
        assert_eq!(last.event_type, "Y");
    }

    #[test]
    fn latest_for_aggregate_returns_none_when_empty() {
        let s = EventStore::open_in_memory().unwrap();
        assert!(s.latest_for_aggregate("nope").unwrap().is_none());
    }
}
```

- [ ] **Step 3: Wire into `store/mod.rs`**

Modify `apps/cashier/src-tauri/src/store/mod.rs`:
```rust
pub mod events;
pub mod master;
pub mod migrations;
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib store::events`
Expected: 5 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/cashier/src-tauri/src/store/events.rs apps/cashier/src-tauri/src/store/mod.rs apps/cashier/src-tauri/src/store/migrations/events/0001_events.sql
git commit -m "feat(cashier): EventStore on separate events.db with day/aggregate queries"
```

---

## Task 7: `services::day_key` — get-or-create wrapped DEK per business day

**Files:**
- Create: `apps/cashier/src-tauri/src/services/mod.rs`
- Create: `apps/cashier/src-tauri/src/services/day_key.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Create module wiring**

Create `apps/cashier/src-tauri/src/services/mod.rs`:
```rust
pub mod day_key;
```

Modify `apps/cashier/src-tauri/src/lib.rs`. Add after `pub mod business_day;`:
```rust
pub mod services;
```

- [ ] **Step 2: Implement service**

Create `apps/cashier/src-tauri/src/services/day_key.rs`:
```rust
use crate::crypto::{Dek, Kek};
use crate::error::AppResult;
use crate::store::master::Master;

/// Get the DEK for `business_day`. If absent, generate a fresh DEK,
/// wrap with KEK, attempt to insert (idempotent). Then unwrap and return.
///
/// This handles the race where two callers both try to create — the second
/// caller's `put_day_key` returns false and they fall through to read the
/// row that the first caller wrote.
pub fn get_or_create(master: &Master, kek: &Kek, business_day: &str) -> AppResult<Dek> {
    if let Some(wrapped) = master.get_day_key(business_day)? {
        return kek.unwrap(&wrapped);
    }
    let dek = Dek::new_random();
    let wrapped = kek.wrap(&dek)?;
    let inserted = master.put_day_key(business_day, &wrapped)?;
    if inserted {
        Ok(dek)
    } else {
        // Lost the race; another caller wrote first. Read theirs.
        let stored = master
            .get_day_key(business_day)?
            .ok_or(crate::error::AppError::NotFound)?;
        kek.unwrap(&stored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_creates_dek() {
        let m = Master::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let dek1 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        let stored = m.get_day_key("2026-04-27").unwrap().unwrap();
        let dek2 = kek.unwrap(&stored).unwrap();
        assert_eq!(dek1.as_bytes(), dek2.as_bytes());
    }

    #[test]
    fn second_call_returns_same_dek() {
        let m = Master::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let d1 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        let d2 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        assert_eq!(d1.as_bytes(), d2.as_bytes());
    }

    #[test]
    fn different_days_yield_different_deks() {
        let m = Master::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let d1 = get_or_create(&m, &kek, "2026-04-27").unwrap();
        let d2 = get_or_create(&m, &kek, "2026-04-28").unwrap();
        assert_ne!(d1.as_bytes(), d2.as_bytes());
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib services::day_key`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/services/mod.rs apps/cashier/src-tauri/src/services/day_key.rs apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): day_key service — get-or-create wrapped DEK"
```

---

## Task 8: Domain events + `event_writer` service

**Files:**
- Create: `apps/cashier/src-tauri/src/domain/mod.rs`
- Create: `apps/cashier/src-tauri/src/domain/event.rs`
- Create: `apps/cashier/src-tauri/src/services/event_writer.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs`

- [ ] **Step 1: Add `serde` derives are already on; nothing to install**

(Both `serde` and `serde_json` are already in `Cargo.toml` from Plan A.)

- [ ] **Step 2: Create domain event enum**

Create `apps/cashier/src-tauri/src/domain/mod.rs`:
```rust
pub mod event;
pub mod session;
pub mod order;
pub mod payment;
```

Create `apps/cashier/src-tauri/src/domain/event.rs`:
```rust
use serde::{Deserialize, Serialize};

/// Top-level discriminator for the `type` column.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventType {
    SessionOpened, SessionClosed, SessionTransferred, SessionMerged, SessionSplit,
    OrderPlaced, OrderItemCancelled, OrderItemReturned,
    PaymentTaken,
}

impl EventType {
    pub fn as_str(self) -> &'static str {
        match self {
            EventType::SessionOpened       => "SessionOpened",
            EventType::SessionClosed       => "SessionClosed",
            EventType::SessionTransferred  => "SessionTransferred",
            EventType::SessionMerged       => "SessionMerged",
            EventType::SessionSplit        => "SessionSplit",
            EventType::OrderPlaced         => "OrderPlaced",
            EventType::OrderItemCancelled  => "OrderItemCancelled",
            EventType::OrderItemReturned   => "OrderItemReturned",
            EventType::PaymentTaken        => "PaymentTaken",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderItemSpec {
    pub product_id: i64,
    pub qty: i64,
    pub unit_price: i64,        // VND
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DomainEvent {
    SessionOpened {
        room_id: Option<i64>,
        table_id: Option<i64>,
        opened_by: i64,
        customer_label: Option<String>,
        team: Option<String>,
    },
    SessionClosed {
        closed_by: i64,
        reason: Option<String>,
    },
    SessionTransferred {
        from_room_id: Option<i64>,
        from_table_id: Option<i64>,
        to_room_id: Option<i64>,
        to_table_id: Option<i64>,
    },
    SessionMerged {
        into_session: String,
        sources: Vec<String>,
    },
    SessionSplit {
        from_session: String,
        new_sessions: Vec<String>,
    },
    OrderPlaced {
        session_id: String,
        order_id: String,
        items: Vec<OrderItemSpec>,
    },
    OrderItemCancelled {
        order_id: String,
        item_index: usize,
        reason: Option<String>,
    },
    OrderItemReturned {
        order_id: String,
        item_index: usize,
        qty: i64,
        reason: Option<String>,
    },
    PaymentTaken {
        session_id: String,
        subtotal: i64,
        discount_pct: u32,
        vat_pct: u32,
        total: i64,
        method: String,
    },
}

impl DomainEvent {
    pub fn event_type(&self) -> EventType {
        match self {
            DomainEvent::SessionOpened {..}      => EventType::SessionOpened,
            DomainEvent::SessionClosed {..}      => EventType::SessionClosed,
            DomainEvent::SessionTransferred {..} => EventType::SessionTransferred,
            DomainEvent::SessionMerged {..}      => EventType::SessionMerged,
            DomainEvent::SessionSplit {..}       => EventType::SessionSplit,
            DomainEvent::OrderPlaced {..}        => EventType::OrderPlaced,
            DomainEvent::OrderItemCancelled {..} => EventType::OrderItemCancelled,
            DomainEvent::OrderItemReturned {..}  => EventType::OrderItemReturned,
            DomainEvent::PaymentTaken {..}       => EventType::PaymentTaken,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_serialization() {
        let e = DomainEvent::SessionOpened {
            room_id: Some(1),
            table_id: None,
            opened_by: 42,
            customer_label: Some("VIP1".into()),
            team: Some("A".into()),
        };
        let s = serde_json::to_string(&e).unwrap();
        let d: DomainEvent = serde_json::from_str(&s).unwrap();
        assert_eq!(e, d);
    }

    #[test]
    fn event_type_strings_stable() {
        assert_eq!(EventType::SessionOpened.as_str(), "SessionOpened");
        assert_eq!(EventType::OrderItemCancelled.as_str(), "OrderItemCancelled");
    }
}
```

- [ ] **Step 3: Create stub session/order/payment files**

Create `apps/cashier/src-tauri/src/domain/session.rs`:
```rust
// projection in Task 9
```
Create `apps/cashier/src-tauri/src/domain/order.rs`:
```rust
// projection in Task 9
```
Create `apps/cashier/src-tauri/src/domain/payment.rs`:
```rust
// projection in Task 9
```

- [ ] **Step 4: Wire domain into lib**

Modify `apps/cashier/src-tauri/src/lib.rs`. Add:
```rust
pub mod domain;
```

- [ ] **Step 5: Implement `event_writer` service**

Modify `apps/cashier/src-tauri/src/services/mod.rs`:
```rust
pub mod day_key;
pub mod event_writer;
```

Create `apps/cashier/src-tauri/src/services/event_writer.rs`:
```rust
use crate::business_day::business_day_of;
use crate::crypto::Kek;
use crate::domain::event::DomainEvent;
use crate::error::AppResult;
use crate::services::day_key;
use crate::store::events::{AppendEvent, EventStore};
use crate::store::master::Master;
use crate::time::Clock;
use chrono::DateTime;
use chrono::FixedOffset;
use chrono::Utc;

// Note: `business_day_of` now requires a `FixedOffset` parameter. `EventWriter` should read
// `business_day_cutoff_hour` and `business_day_tz_offset_seconds` from `setting` (or accept
// them as fields) and pass through.
pub struct EventWriter<'a> {
    pub master: &'a Master,
    pub events: &'a EventStore,
    pub kek: &'a Kek,
    pub clock: &'a dyn Clock,
    pub cutoff_hour: u32,
    pub tz: FixedOffset,
}

pub struct WriteCtx<'a> {
    pub aggregate_id: &'a str,
    pub actor_staff: Option<i64>,
    /// Override "now" for testability. Production = None (use clock).
    pub at: Option<DateTime<Utc>>,
}

impl<'a> EventWriter<'a> {
    pub fn write(&self, ctx: WriteCtx<'_>, ev: &DomainEvent) -> AppResult<i64> {
        let now = ctx.at.unwrap_or_else(|| self.clock.now());
        let ts = now.timestamp_millis();
        let day = business_day_of(now, self.cutoff_hour);
        let dek = day_key::get_or_create(self.master, self.kek, &day)?;

        let payload = serde_json::to_vec(ev).map_err(|e| {
            crate::error::AppError::Validation(format!("serialize event: {e}"))
        })?;
        // AAD binds ciphertext to {day, type, aggregate_id} so a swap
        // across days/aggregates fails GCM auth.
        let aad = format!("{day}|{}|{}", ev.event_type().as_str(), ctx.aggregate_id);
        let blob = dek.encrypt(&payload, aad.as_bytes())?;

        self.events.append(AppendEvent {
            business_day: &day,
            ts,
            event_type: ev.event_type().as_str(),
            aggregate_id: ctx.aggregate_id,
            actor_staff: ctx.actor_staff,
            payload_enc: &blob,
            key_id: &day,
        })
    }

    pub fn read_decrypted(&self, row: &crate::store::events::EventRow) -> AppResult<DomainEvent> {
        let wrapped = self
            .master
            .get_day_key(&row.key_id)?
            .ok_or(crate::error::AppError::NotFound)?;
        let dek = self.kek.unwrap(&wrapped)?;
        let aad = format!("{}|{}|{}", row.business_day, row.event_type, row.aggregate_id);
        let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
        let ev: DomainEvent = serde_json::from_slice(&pt)
            .map_err(|e| crate::error::AppError::Validation(format!("deserialize: {e}")))?;
        Ok(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Kek;
    use crate::domain::event::DomainEvent;
    use crate::store::events::EventStore;
    use crate::store::master::Master;
    use crate::time::test_support::MockClock;

    fn rig() -> (Master, EventStore, Kek, MockClock) {
        let master = Master::open_in_memory().unwrap();
        let events = EventStore::open_in_memory().unwrap();
        let kek = Kek::new_random();
        let clock = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
        (master, events, kek, clock)
    }

    #[test]
    fn write_then_read_roundtrip() {
        let (master, events, kek, clock) = rig();
        let writer = EventWriter { master: &master, events: &events, kek: &kek, clock: &clock, cutoff_hour: 11 };

        let ev = DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: 7,
            customer_label: Some("L".into()), team: None,
        };
        let id = writer
            .write(WriteCtx { aggregate_id: "sess-1", actor_staff: Some(7), at: None }, &ev)
            .unwrap();
        assert!(id > 0);

        let rows = events.list_for_aggregate("sess-1").unwrap();
        assert_eq!(rows.len(), 1);
        let decoded = writer.read_decrypted(&rows[0]).unwrap();
        assert_eq!(decoded, ev);
    }

    #[test]
    fn cross_midnight_event_belongs_to_opening_day() {
        let (master, events, kek, clock) = rig();
        let writer = EventWriter { master: &master, events: &events, kek: &kek, clock: &clock, cutoff_hour: 11 };
        // 2026-04-28 03:00 with cutoff 11 → business_day 2026-04-27
        let at = chrono::TimeZone::with_ymd_and_hms(&Utc, 2026, 4, 28, 3, 0, 0).unwrap();
        let ev = DomainEvent::SessionClosed { closed_by: 1, reason: None };
        writer.write(WriteCtx { aggregate_id: "x", actor_staff: None, at: Some(at) }, &ev).unwrap();
        assert_eq!(events.count_for_day("2026-04-27").unwrap(), 1);
        assert_eq!(events.count_for_day("2026-04-28").unwrap(), 0);
    }

    #[test]
    fn aad_tamper_fails_decrypt() {
        let (master, events, kek, clock) = rig();
        let writer = EventWriter { master: &master, events: &events, kek: &kek, clock: &clock, cutoff_hour: 11 };
        let ev = DomainEvent::SessionClosed { closed_by: 1, reason: None };
        writer.write(WriteCtx { aggregate_id: "real", actor_staff: None, at: None }, &ev).unwrap();
        let mut rows = events.list_for_aggregate("real").unwrap();
        // Forge: pretend the row's aggregate_id was something else; the AAD bound to
        // the ORIGINAL aggregate must now fail.
        rows[0].aggregate_id = "forged".into();
        assert!(writer.read_decrypted(&rows[0]).is_err());
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib`
Expected: all prior + new (3 from event, 3 from event_writer) pass.

- [ ] **Step 7: Commit**

```bash
git add apps/cashier/src-tauri/src/domain apps/cashier/src-tauri/src/services apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): DomainEvent enum + EventWriter (encrypt/decrypt with AAD-bound payloads)"
```

---

## Task 9: Projections — `SessionState`, `OrderState`, `PaymentState`

**Files:**
- Modify: `apps/cashier/src-tauri/src/domain/session.rs`
- Modify: `apps/cashier/src-tauri/src/domain/order.rs`
- Modify: `apps/cashier/src-tauri/src/domain/payment.rs`

- [ ] **Step 1: Implement `SessionState`**

Replace `apps/cashier/src-tauri/src/domain/session.rs`:
```rust
use crate::domain::event::DomainEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionStatus {
    Open,
    Closed,
    Merged { into: String },
    Split,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionState {
    pub session_id: String,
    pub status: SessionStatus,
    pub room_id: Option<i64>,
    pub table_id: Option<i64>,
    pub opened_by: i64,
    pub customer_label: Option<String>,
    pub team: Option<String>,
}

/// Fold events for a single session into its current state.
/// `events` MUST be ordered by `id ASC` and all share the same `aggregate_id`.
/// Returns None if no SessionOpened event found.
pub fn fold(session_id: &str, events: &[DomainEvent]) -> Option<SessionState> {
    let mut state: Option<SessionState> = None;
    for ev in events {
        match ev {
            DomainEvent::SessionOpened { room_id, table_id, opened_by, customer_label, team } => {
                state = Some(SessionState {
                    session_id: session_id.to_string(),
                    status: SessionStatus::Open,
                    room_id: *room_id,
                    table_id: *table_id,
                    opened_by: *opened_by,
                    customer_label: customer_label.clone(),
                    team: team.clone(),
                });
            }
            DomainEvent::SessionClosed { .. } => {
                if let Some(s) = state.as_mut() {
                    s.status = SessionStatus::Closed;
                }
            }
            DomainEvent::SessionTransferred { to_room_id, to_table_id, .. } => {
                if let Some(s) = state.as_mut() {
                    s.room_id = *to_room_id;
                    s.table_id = *to_table_id;
                }
            }
            DomainEvent::SessionMerged { into_session, sources } => {
                if let Some(s) = state.as_mut() {
                    if sources.iter().any(|src| src == &s.session_id) {
                        s.status = SessionStatus::Merged { into: into_session.clone() };
                    }
                }
            }
            DomainEvent::SessionSplit { from_session, .. } => {
                if let Some(s) = state.as_mut() {
                    if from_session == &s.session_id {
                        s.status = SessionStatus::Split;
                    }
                }
            }
            _ => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened() -> DomainEvent {
        DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: 7,
            customer_label: Some("L".into()), team: None,
        }
    }

    #[test]
    fn no_events_yields_none() {
        assert!(fold("s", &[]).is_none());
    }

    #[test]
    fn opened_yields_open_state() {
        let s = fold("s", &[opened()]).unwrap();
        assert_eq!(s.status, SessionStatus::Open);
        assert_eq!(s.room_id, Some(1));
        assert_eq!(s.opened_by, 7);
    }

    #[test]
    fn open_then_close() {
        let evs = vec![opened(), DomainEvent::SessionClosed { closed_by: 7, reason: None }];
        assert_eq!(fold("s", &evs).unwrap().status, SessionStatus::Closed);
    }

    #[test]
    fn transfer_updates_target() {
        let evs = vec![
            opened(),
            DomainEvent::SessionTransferred {
                from_room_id: Some(1), from_table_id: None,
                to_room_id: None, to_table_id: Some(5),
            },
        ];
        let s = fold("s", &evs).unwrap();
        assert_eq!(s.room_id, None);
        assert_eq!(s.table_id, Some(5));
    }

    #[test]
    fn merge_marks_source_as_merged() {
        let evs = vec![
            opened(),
            DomainEvent::SessionMerged {
                into_session: "target".into(),
                sources: vec!["s".into()],
            },
        ];
        match fold("s", &evs).unwrap().status {
            SessionStatus::Merged { into } => assert_eq!(into, "target"),
            other => panic!("expected Merged, got {other:?}"),
        }
    }

    #[test]
    fn split_marks_source_as_split() {
        let evs = vec![
            opened(),
            DomainEvent::SessionSplit {
                from_session: "s".into(),
                new_sessions: vec!["a".into(), "b".into()],
            },
        ];
        assert_eq!(fold("s", &evs).unwrap().status, SessionStatus::Split);
    }
}
```

- [ ] **Step 2: Implement `OrderState`**

Replace `apps/cashier/src-tauri/src/domain/order.rs`:
```rust
use crate::domain::event::{DomainEvent, OrderItemSpec};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderItem {
    pub spec: OrderItemSpec,
    pub cancelled: bool,
    pub returned_qty: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OrderState {
    pub order_id: String,
    pub session_id: String,
    pub items: Vec<OrderItem>,
}

impl OrderState {
    pub fn live_subtotal(&self) -> i64 {
        self.items.iter()
            .filter(|i| !i.cancelled)
            .map(|i| {
                let net_qty = (i.spec.qty - i.returned_qty).max(0);
                net_qty * i.spec.unit_price
            })
            .sum()
    }
}

/// Fold events tagged with this order_id (already filtered by caller) into state.
/// Returns None if no OrderPlaced.
pub fn fold(order_id: &str, events: &[DomainEvent]) -> Option<OrderState> {
    let mut state: Option<OrderState> = None;
    for ev in events {
        match ev {
            DomainEvent::OrderPlaced { session_id, order_id: oid, items } if oid == order_id => {
                state = Some(OrderState {
                    order_id: order_id.into(),
                    session_id: session_id.clone(),
                    items: items.iter().cloned().map(|spec| OrderItem {
                        spec, cancelled: false, returned_qty: 0,
                    }).collect(),
                });
            }
            DomainEvent::OrderItemCancelled { order_id: oid, item_index, .. } if oid == order_id => {
                if let Some(s) = state.as_mut() {
                    if let Some(it) = s.items.get_mut(*item_index) {
                        it.cancelled = true;
                    }
                }
            }
            DomainEvent::OrderItemReturned { order_id: oid, item_index, qty, .. } if oid == order_id => {
                if let Some(s) = state.as_mut() {
                    if let Some(it) = s.items.get_mut(*item_index) {
                        it.returned_qty += qty;
                    }
                }
            }
            _ => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placed() -> DomainEvent {
        DomainEvent::OrderPlaced {
            session_id: "s".into(),
            order_id: "o".into(),
            items: vec![
                OrderItemSpec { product_id: 1, qty: 2, unit_price: 50_000, note: None },
                OrderItemSpec { product_id: 2, qty: 1, unit_price: 100_000, note: None },
            ],
        }
    }

    #[test]
    fn placed_yields_state_with_items() {
        let s = fold("o", &[placed()]).unwrap();
        assert_eq!(s.items.len(), 2);
        assert_eq!(s.live_subtotal(), 2 * 50_000 + 1 * 100_000);
    }

    #[test]
    fn cancel_excludes_item() {
        let evs = vec![
            placed(),
            DomainEvent::OrderItemCancelled { order_id: "o".into(), item_index: 1, reason: None },
        ];
        let s = fold("o", &evs).unwrap();
        assert!(!s.items[0].cancelled);
        assert!(s.items[1].cancelled);
        assert_eq!(s.live_subtotal(), 2 * 50_000);
    }

    #[test]
    fn return_reduces_subtotal() {
        let evs = vec![
            placed(),
            DomainEvent::OrderItemReturned { order_id: "o".into(), item_index: 0, qty: 1, reason: None },
        ];
        let s = fold("o", &evs).unwrap();
        assert_eq!(s.items[0].returned_qty, 1);
        assert_eq!(s.live_subtotal(), 1 * 50_000 + 1 * 100_000);
    }

    #[test]
    fn unrelated_order_events_ignored() {
        let evs = vec![
            placed(),
            DomainEvent::OrderItemCancelled { order_id: "different".into(), item_index: 0, reason: None },
        ];
        let s = fold("o", &evs).unwrap();
        assert!(!s.items[0].cancelled);
    }
}
```

- [ ] **Step 3: Implement `PaymentState`**

Replace `apps/cashier/src-tauri/src/domain/payment.rs`:
```rust
use crate::domain::event::DomainEvent;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PaymentState {
    pub session_id: String,
    pub subtotal: i64,
    pub discount_pct: u32,
    pub vat_pct: u32,
    pub total: i64,
    pub method: String,
}

/// Fold returns the FIRST PaymentTaken for `session_id`. Subsequent payment
/// events are an invariant violation and the caller (event writer) must
/// reject them — this fold ignores them so projections stay deterministic.
pub fn fold(session_id: &str, events: &[DomainEvent]) -> Option<PaymentState> {
    for ev in events {
        if let DomainEvent::PaymentTaken { session_id: sid, subtotal, discount_pct, vat_pct, total, method } = ev {
            if sid == session_id {
                return Some(PaymentState {
                    session_id: sid.clone(),
                    subtotal: *subtotal,
                    discount_pct: *discount_pct,
                    vat_pct: *vat_pct,
                    total: *total,
                    method: method.clone(),
                });
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_payment_yields_none() {
        assert!(fold("s", &[]).is_none());
    }

    #[test]
    fn first_payment_wins() {
        let evs = vec![
            DomainEvent::PaymentTaken { session_id: "s".into(), subtotal: 100, discount_pct: 0, vat_pct: 8, total: 108, method: "cash".into() },
            DomainEvent::PaymentTaken { session_id: "s".into(), subtotal: 999, discount_pct: 50, vat_pct: 0, total: 500, method: "card".into() },
        ];
        let p = fold("s", &evs).unwrap();
        assert_eq!(p.total, 108);
        assert_eq!(p.method, "cash");
    }

    #[test]
    fn other_session_ignored() {
        let evs = vec![
            DomainEvent::PaymentTaken { session_id: "other".into(), subtotal: 100, discount_pct: 0, vat_pct: 8, total: 108, method: "cash".into() },
        ];
        assert!(fold("s", &evs).is_none());
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib domain`
Expected: 6 + 4 + 3 = 13 new tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/cashier/src-tauri/src/domain
git commit -m "feat(cashier): SessionState/OrderState/PaymentState projections + folds"
```

---

## Task 10: Integration test — full session lifecycle

**Files:**
- Create: `apps/cashier/src-tauri/tests/projection_integration.rs`

- [ ] **Step 1: Write end-to-end test**

Create `apps/cashier/src-tauri/tests/projection_integration.rs`:
```rust
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::{DomainEvent, OrderItemSpec};
use cashier_lib::domain::{order, payment, session};
use cashier_lib::services::event_writer::{EventWriter, WriteCtx};
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;

#[test]
fn full_session_lifecycle_replays_to_expected_state() {
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 14, 0, 0);
    let writer = EventWriter {
        master: &master, events: &events, kek: &kek, clock: &clock, cutoff_hour: 11,
    };

    // 1. Open session
    writer.write(
        WriteCtx { aggregate_id: "sess-1", actor_staff: Some(7), at: None },
        &DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: 7,
            customer_label: Some("VIP".into()), team: Some("A".into()),
        },
    ).unwrap();

    clock.advance_minutes(5);

    // 2. Place order
    writer.write(
        WriteCtx { aggregate_id: "ord-1", actor_staff: Some(7), at: None },
        &DomainEvent::OrderPlaced {
            session_id: "sess-1".into(),
            order_id: "ord-1".into(),
            items: vec![
                OrderItemSpec { product_id: 10, qty: 2, unit_price: 50_000, note: None },
                OrderItemSpec { product_id: 11, qty: 1, unit_price: 200_000, note: Some("ice".into()) },
            ],
        },
    ).unwrap();

    clock.advance_minutes(10);

    // 3. Cancel one item
    writer.write(
        WriteCtx { aggregate_id: "ord-1", actor_staff: Some(7), at: None },
        &DomainEvent::OrderItemCancelled {
            order_id: "ord-1".into(), item_index: 1, reason: Some("returned to bar".into()),
        },
    ).unwrap();

    clock.advance_minutes(60);

    // 4. Pay
    writer.write(
        WriteCtx { aggregate_id: "pay-1", actor_staff: Some(7), at: None },
        &DomainEvent::PaymentTaken {
            session_id: "sess-1".into(),
            subtotal: 100_000, discount_pct: 0, vat_pct: 8, total: 108_000,
            method: "cash".into(),
        },
    ).unwrap();

    clock.advance_minutes(1);

    // 5. Close
    writer.write(
        WriteCtx { aggregate_id: "sess-1", actor_staff: Some(7), at: None },
        &DomainEvent::SessionClosed { closed_by: 7, reason: None },
    ).unwrap();

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
    assert_eq!(s.room_id, Some(1));

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
fn shred_day_renders_payloads_unreadable() {
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 14, 0, 0);
    let writer = EventWriter {
        master: &master, events: &events, kek: &kek, clock: &clock, cutoff_hour: 11,
    };

    writer.write(
        WriteCtx { aggregate_id: "sess-1", actor_staff: None, at: None },
        &DomainEvent::SessionClosed { closed_by: 1, reason: None },
    ).unwrap();

    let row = events.list_for_day("2026-04-27").unwrap().remove(0);
    // Decrypt works pre-shred
    assert!(writer.read_decrypted(&row).is_ok());

    // Shred the wrapped DEK
    assert!(master.delete_day_key("2026-04-27").unwrap());

    // Decrypt now fails — no DEK to unwrap
    assert!(writer.read_decrypted(&row).is_err());
}
```

- [ ] **Step 2: Run integration test**

Run: `cd apps/cashier/src-tauri && cargo test --test projection_integration`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add apps/cashier/src-tauri/tests/projection_integration.rs
git commit -m "test(cashier): full session lifecycle integration + shred verification"
```

---

## Task 11: Final gate

**Files:** none

- [ ] **Step 1: Full Rust suite**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: ~50+ tests, all pass.

- [ ] **Step 2: Clippy**

Run: `cargo clippy --all-targets -- -D warnings`
Expected: clean.

- [ ] **Step 3: Format check**

Run: `cargo fmt --check`
Expected: clean. Run `cargo fmt` if needed.

- [ ] **Step 4: TS typecheck (sanity)**

Run: `pnpm --filter @tableorder/cashier typecheck`
Expected: clean.

- [ ] **Step 5: Final fmt commit if needed**

```bash
git status
# if any:
git add -u && git commit -m "chore(cashier): cargo fmt"
```

---

## Done

End state:
- `Clock` trait + `MockClock`, `business_day_of` math
- Reusable migration runner pointed at `master/` and `events/` SQL dirs
- `Master::with_tx` for caller transactions
- `EventStore` on its own `events.db` with append/list/delete-day/vacuum
- `services::day_key::get_or_create` — race-safe wrapped-DEK lifecycle
- `DomainEvent` enum + `EventWriter` with AAD-bound `nonce||ct||tag` payloads
- `SessionState` / `OrderState` / `PaymentState` projection folds
- Integration test proves full lifecycle replays + shred renders payloads unreadable

Ready for **Plan C** (auth + ACL + axum HTTP/WS + Tauri commands).
