# UTC Key Rotation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `services::day_key` (business-day-keyed) with a `KeyManager` (UTC-day-keyed) backed by a new `dek` table. Add a `RotationScheduler` tokio task that ensures today's UTC DEK exists and prunes keys older than 3 UTC days. Move `event` row deletion to the EOD pipeline (cutoff-driven). Add `GET /admin/keys` for ops visibility. Drop `day_key` table — pre-prod, no data preservation.

**Architecture:** Two independent lifecycles. Crypto lifecycle is UTC-driven (rotation service, hard 72h key TTL); data lifecycle is cutoff-driven (EOD writes report + deletes that business_day's event rows). AAD shape unchanged: `{business_day}|{event_type}|{aggregate_id}|{key_id}` — `business_day` is the reporting tag, `key_id` is the crypto tag (now `utc_day`, no longer always equal to `business_day`).

**Spec:** `docs/superpowers/specs/2026-04-28-utc-key-rotation-design.md`

**Tech stack additions:** none (chrono already added by Plan F).

**Out of scope:**
- Tor / `arti` / `.onion` — separate spec if pursued.
- Re-encryption of historic events on rotation.
- Configurable retention (3 days hard-coded).
- Data migration from `day_key` (pre-prod, drop-and-recreate).

---

## File Structure

```
apps/cashier/src-tauri/src/
  services/
    day_key.rs                            # DELETED
    key_manager.rs                        # NEW — KeyManager (utc_day-keyed get_or_create + dek_for + rotate)
    utc_day.rs                            # NEW — utc_day_of(ts_ms) helper + tests
  rotation/
    mod.rs                                # NEW — pub use scheduler
    scheduler.rs                          # NEW — tokio task: ensure today + prune > 3 days, on UTC midnight
  app_state.rs                            # MODIFIED — drop direct master/kek calls for keys; expose Arc<KeyManager>
  lib.rs                                  # MODIFIED — construct KeyManager, spawn rotation::scheduler::spawn
  services/event_service.rs               # MODIFIED — encrypt with key_manager.current_dek; key_id = utc_day
  store/master.rs                         # MODIFIED — replace day_key fns with dek fns (get/put/delete/list)
  store/migrations/master/
    0006_dek_rotation.sql                 # NEW — drop day_key, create dek
  eod/
    runner.rs                             # MODIFIED — drop DELETE FROM day_key; add DELETE FROM event WHERE business_day = ?
    builder.rs                            # unchanged behaviour, but verify cross-utc-day events still bucket correctly
  http/routes/
    admin.rs                              # MODIFIED — add GET /admin/keys (Owner-only)

packages/shared/src/schemas/
  admin.ts                                # MODIFIED — add DekInfo schema for /admin/keys
```

Decisions:
- **`utc_day_of(ts_ms)`** lives in its own file and is trivially `chrono::Utc.timestamp_millis_opt(ts).unwrap().format("%Y-%m-%d")`. Distinct from `business_day_of` to make the call site read clearly.
- **Retention = 3 days** is `const KEY_TTL_DAYS: i64 = 3;` in `key_manager.rs`. "Older" means `utc_day < (today_utc - 3 days)`.
- **KeyManager is a struct** with `Arc<Mutex<Master>>` + `Arc<Kek>`, identical wiring style to existing services. No globals.
- **EventService takes `Arc<KeyManager>`** (was: ad-hoc `master.lock(); day_key::get_or_create(...)`). Cleaner, easier to mock in tests.
- **`event.key_id` semantics shift in code only.** Schema unchanged. Encrypt path writes `utc_day(now)`; decrypt path looks up by `key_id` against `dek` (was `day_key`). AAD format string unchanged so existing AAD-tamper tests still exercise the same surface.
- **Event-row deletion at EOD** runs INSIDE the runner's existing transaction (after `daily_report` upsert, after dek-related code is gone). Failure → transaction rollback → report row also reverts → next run retries from scratch.
- **Migration is destructive.** Pre-prod. No try-to-preserve-data step. Anyone with pre-existing `day_key` rows loses readability; acceptable.
- **Crypto-shred test that today asserts "after EOD, events are unreadable"** — split into two tests: (a) "after rotation prunes the key, decrypt fails" and (b) "after EOD, event rows are deleted from the table." Different concerns, separate tests.

---

## Task 1: utc_day helper

**Files:**
- Create: `apps/cashier/src-tauri/src/services/utc_day.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs` (add `pub mod utc_day;`)

- [ ] **Step 1: tests first**

```rust
// apps/cashier/src-tauri/src/services/utc_day.rs
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(y: i32, m: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::Utc.with_ymd_and_hms(y, m, d, h, mi, 0).unwrap().timestamp_millis()
    }

    #[test] fn epoch_is_1970_01_01() { assert_eq!(utc_day_of(0), "1970-01-01"); }

    #[test] fn just_before_midnight_is_today() {
        assert_eq!(utc_day_of(ts(2026, 4, 28, 23, 59)), "2026-04-28");
    }

    #[test] fn at_midnight_starts_new_day() {
        assert_eq!(utc_day_of(ts(2026, 4, 29, 0, 0)), "2026-04-29");
    }

    #[test] fn local_offset_does_not_affect_utc_day() {
        // 2026-04-28 23:00 UTC == 2026-04-29 06:00 +07; UTC day is the 28th.
        assert_eq!(utc_day_of(ts(2026, 4, 28, 23, 0)), "2026-04-28");
    }

    #[test] fn next_utc_midnight_after_now() {
        let now = ts(2026, 4, 28, 14, 30);
        let next = next_utc_midnight_ms(now);
        assert_eq!(next, ts(2026, 4, 29, 0, 0));
    }

    #[test] fn at_utc_midnight_returns_next_one() {
        let now = ts(2026, 4, 29, 0, 0);
        let next = next_utc_midnight_ms(now);
        assert_eq!(next, ts(2026, 4, 30, 0, 0));
    }

    #[test] fn days_ago_subtracts_correctly() {
        assert_eq!(days_ago("2026-04-28", 3), "2026-04-25");
        assert_eq!(days_ago("2026-04-28", 0), "2026-04-28");
    }
}
```

- [ ] **Step 2: implementation**

```rust
// apps/cashier/src-tauri/src/services/utc_day.rs
use chrono::{Duration, NaiveDate, TimeZone, Utc};

pub fn utc_day_of(ts_ms: i64) -> String {
    Utc.timestamp_millis_opt(ts_ms).unwrap().format("%Y-%m-%d").to_string()
}

pub fn next_utc_midnight_ms(now_ms: i64) -> i64 {
    let dt = Utc.timestamp_millis_opt(now_ms).unwrap();
    let next = (dt.date_naive() + Duration::days(1)).and_hms_opt(0, 0, 0).unwrap();
    Utc.from_utc_datetime(&next).timestamp_millis()
}

pub fn days_ago(day: &str, n: i64) -> String {
    let d = NaiveDate::parse_from_str(day, "%Y-%m-%d").unwrap();
    (d - Duration::days(n)).format("%Y-%m-%d").to_string()
}
```

- [ ] **Step 3: cargo test services::utc_day**

Expected: 7/7 pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(services): utc_day helper (utc_day_of, next_utc_midnight_ms, days_ago)"
```

---

## Task 2: dek migration + Master CRUD

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0006_dek_rotation.sql`
- Modify: `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: migration**

```sql
-- 0006_dek_rotation.sql
DROP TABLE IF EXISTS day_key;

CREATE TABLE dek (
  utc_day     TEXT PRIMARY KEY,         -- 'YYYY-MM-DD' UTC
  wrapped_dek BLOB NOT NULL,
  created_at  INTEGER NOT NULL
);
```

- [ ] **Step 2: Master fns — replace day_key with dek**

In `master.rs`, remove `get_day_key`, `put_day_key`, `delete_day_key`, `list_active_business_days`. Add:

```rust
impl Master {
    pub fn get_dek(&self, utc_day: &str) -> AppResult<Option<Vec<u8>>> {
        let conn = self.conn.lock().unwrap();
        match conn.query_row(
            "SELECT wrapped_dek FROM dek WHERE utc_day = ?1",
            params![utc_day],
            |r| r.get::<_, Vec<u8>>(0),
        ) {
            Ok(b) => Ok(Some(b)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Returns true if a row was inserted (false on conflict).
    pub fn put_dek(&self, utc_day: &str, wrapped: &[u8], now_ms: i64) -> AppResult<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute(
            "INSERT OR IGNORE INTO dek(utc_day, wrapped_dek, created_at) VALUES (?1, ?2, ?3)",
            params![utc_day, wrapped, now_ms],
        )?;
        Ok(n == 1)
    }

    pub fn delete_dek(&self, utc_day: &str) -> AppResult<bool> {
        let conn = self.conn.lock().unwrap();
        let n = conn.execute("DELETE FROM dek WHERE utc_day = ?1", params![utc_day])?;
        Ok(n > 0)
    }

    pub fn list_dek_days(&self) -> AppResult<Vec<DekInfo>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT utc_day, created_at FROM dek ORDER BY utc_day DESC")?;
        let rows = stmt.query_map([], |r| Ok(DekInfo { utc_day: r.get(0)?, created_at: r.get(1)? }))?;
        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    pub fn delete_deks_older_than(&self, oldest_keep: &str) -> AppResult<Vec<String>> {
        let conn = self.conn.lock().unwrap();
        let mut stmt = conn.prepare("SELECT utc_day FROM dek WHERE utc_day < ?1")?;
        let to_delete: Vec<String> = stmt
            .query_map(params![oldest_keep], |r| r.get::<_, String>(0))?
            .collect::<Result<_, _>>()?;
        for d in &to_delete {
            conn.execute("DELETE FROM dek WHERE utc_day = ?1", params![d])?;
        }
        Ok(to_delete)
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DekInfo {
    pub utc_day: String,
    pub created_at: i64,
}
```

- [ ] **Step 3: tests**

```rust
#[test] fn put_then_get_round_trips() { /* insert blob, read back equal */ }
#[test] fn put_dek_returns_false_on_conflict() { /* second insert same day */ }
#[test] fn delete_deks_older_than_returns_deleted_days() {
    // seed 2026-04-25..2026-04-29; delete_deks_older_than("2026-04-27")
    // expect deleted = ["2026-04-25","2026-04-26"]; remaining = 27,28,29
}
#[test] fn list_dek_days_returns_desc_order() { /* … */ }
```

- [ ] **Step 4: cargo test store::master::dek**

Expected: 4/4 pass; existing master tests for day_key fns no longer compile (they were deleted with the fns) — confirm 0 compile errors after the deletion.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(store): replace day_key table with dek (utc_day-keyed); CRUD + retention sweep"
```

---

## Task 3: KeyManager service

**Files:**
- Delete: `apps/cashier/src-tauri/src/services/day_key.rs`
- Create: `apps/cashier/src-tauri/src/services/key_manager.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs` (drop `day_key`, add `key_manager`)

- [ ] **Step 1: tests first**

```rust
// apps/cashier/src-tauri/src/services/key_manager.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::crypto::Kek;
    use crate::store::master::Master;
    use std::sync::{Arc, Mutex};

    fn rig() -> KeyManager {
        let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
        let kek = Arc::new(Kek::new_random());
        KeyManager::new(master, kek)
    }

    fn ts(y: i32, m: u32, d: u32) -> i64 {
        use chrono::TimeZone;
        chrono::Utc.with_ymd_and_hms(y, m, d, 12, 0, 0).unwrap().timestamp_millis()
    }

    #[test]
    fn current_dek_creates_on_first_call() {
        let km = rig();
        let d1 = km.current_dek(ts(2026, 4, 28)).unwrap();
        let d2 = km.current_dek(ts(2026, 4, 28)).unwrap();
        assert_eq!(d1.as_bytes(), d2.as_bytes());
    }

    #[test]
    fn current_dek_differs_per_utc_day() {
        let km = rig();
        let d1 = km.current_dek(ts(2026, 4, 28)).unwrap();
        let d2 = km.current_dek(ts(2026, 4, 29)).unwrap();
        assert_ne!(d1.as_bytes(), d2.as_bytes());
    }

    #[test]
    fn dek_for_returns_key_expired_if_missing() {
        let km = rig();
        let err = km.dek_for("1999-01-01").unwrap_err();
        assert!(matches!(err, AppError::Crypto(_)));
        assert!(err.to_string().contains("key expired"));
    }

    #[test]
    fn rotate_creates_today_and_prunes_older_than_3_days() {
        let km = rig();
        // Pre-seed 5 days
        for d in &["2026-04-22","2026-04-23","2026-04-24","2026-04-25","2026-04-26"] {
            // Force-create at that utc_day
            km.current_dek_at(d, ts(2026,4,22)).unwrap();
            let _ = km.current_dek_at(d, ts(2026,4,22));
        }
        // Today = 2026-04-28; keep [-3..0] => 2026-04-25, 26, 27, 28 (4 days). Drop 22,23,24.
        let report = km.rotate(ts(2026, 4, 28)).unwrap();
        assert_eq!(report.deleted, vec!["2026-04-22","2026-04-23","2026-04-24"]);
        assert!(report.created_today);
    }

    #[test]
    fn rotate_idempotent() {
        let km = rig();
        let r1 = km.rotate(ts(2026, 4, 28)).unwrap();
        let r2 = km.rotate(ts(2026, 4, 28)).unwrap();
        assert!(r1.created_today);
        assert!(!r2.created_today);
        assert!(r2.deleted.is_empty());
    }
}
```

(`current_dek_at` is a test-only ctor that lets us seed historic-dated keys. Add it `#[cfg(test)] pub fn current_dek_at(&self, utc_day: &str, now_ms: i64) -> AppResult<Dek>`.)

- [ ] **Step 2: implementation**

```rust
// apps/cashier/src-tauri/src/services/key_manager.rs
use crate::crypto::{Dek, Kek};
use crate::error::{AppError, AppResult};
use crate::services::utc_day::{days_ago, utc_day_of};
use crate::store::master::Master;
use std::sync::{Arc, Mutex};

pub const KEY_TTL_DAYS: i64 = 3;

pub struct KeyManager {
    master: Arc<Mutex<Master>>,
    kek: Arc<Kek>,
}

#[derive(Debug)]
pub struct RotationReport {
    pub today: String,
    pub created_today: bool,
    pub deleted: Vec<String>,
}

impl KeyManager {
    pub fn new(master: Arc<Mutex<Master>>, kek: Arc<Kek>) -> Self {
        Self { master, kek }
    }

    pub fn current_dek(&self, now_ms: i64) -> AppResult<Dek> {
        let day = utc_day_of(now_ms);
        self.get_or_create(&day, now_ms)
    }

    pub fn dek_for(&self, utc_day: &str) -> AppResult<Dek> {
        let m = self.master.lock().unwrap();
        let wrapped = m
            .get_dek(utc_day)?
            .ok_or_else(|| AppError::Crypto(format!("key expired for {utc_day}")))?;
        self.kek.unwrap(&wrapped)
    }

    pub fn rotate(&self, now_ms: i64) -> AppResult<RotationReport> {
        let today = utc_day_of(now_ms);
        let created_today = self.get_or_create_inserted(&today, now_ms)?;
        let oldest_keep = days_ago(&today, KEY_TTL_DAYS);
        let deleted = {
            let m = self.master.lock().unwrap();
            m.delete_deks_older_than(&oldest_keep)?
        };
        Ok(RotationReport { today, created_today, deleted })
    }

    fn get_or_create(&self, utc_day: &str, now_ms: i64) -> AppResult<Dek> {
        let m = self.master.lock().unwrap();
        if let Some(wrapped) = m.get_dek(utc_day)? {
            return self.kek.unwrap(&wrapped);
        }
        let dek = Dek::new_random();
        let wrapped = self.kek.wrap(&dek)?;
        let inserted = m.put_dek(utc_day, &wrapped, now_ms)?;
        if inserted {
            Ok(dek)
        } else {
            // Race lost; read the winner.
            let stored = m.get_dek(utc_day)?.ok_or(AppError::NotFound)?;
            self.kek.unwrap(&stored)
        }
    }

    /// Returns true if this call inserted the row (vs found existing).
    fn get_or_create_inserted(&self, utc_day: &str, now_ms: i64) -> AppResult<bool> {
        let m = self.master.lock().unwrap();
        if m.get_dek(utc_day)?.is_some() { return Ok(false); }
        let dek = Dek::new_random();
        let wrapped = self.kek.wrap(&dek)?;
        Ok(m.put_dek(utc_day, &wrapped, now_ms)?)
    }

    #[cfg(test)]
    pub fn current_dek_at(&self, utc_day: &str, now_ms: i64) -> AppResult<Dek> {
        self.get_or_create(utc_day, now_ms)
    }
}
```

- [ ] **Step 3: services/mod.rs**

Replace `pub mod day_key;` with `pub mod key_manager;` (keep `pub mod utc_day;`).

- [ ] **Step 4: cargo test services::key_manager**

Expected: 5/5 pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(services): KeyManager — UTC daily DEKs, 3-day retention, rotate()"
```

---

## Task 4: EventService uses KeyManager

**Files:**
- Modify: `apps/cashier/src-tauri/src/services/event_service.rs`
- Modify: `apps/cashier/src-tauri/src/app_state.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: `apps/cashier/src-tauri/src/cli.rs`

- [ ] **Step 1: EventService swap**

Drop the `master`/`kek` fields. Add `pub key_manager: Arc<KeyManager>`. In `write`:

```rust
let now = ctx.at.unwrap_or_else(|| self.clock.now());
let ts = now.timestamp_millis();
let business_day = business_day_of(now, self.tz, self.cutoff_hour);
let utc_day = crate::services::utc_day::utc_day_of(ts);
let dek = self.key_manager.current_dek(ts)?;

let payload = serde_json::to_vec(ev).map_err(|e| AppError::Internal(format!("serialize: {e}")))?;
let aad = format!("{business_day}|{}|{}|{utc_day}", ev.event_type().as_str(), ctx.aggregate_id);
let blob = dek.encrypt(&payload, aad.as_bytes())?;

self.events.append(AppendEvent {
    business_day: &business_day,
    ts,
    event_type: ev.event_type().as_str(),
    aggregate_id: ctx.aggregate_id,
    actor_staff: ctx.actor_staff,
    actor_name: ctx.actor_name,
    override_staff_id: ctx.override_staff_id,
    override_staff_name: ctx.override_staff_name,
    payload_enc: &blob,
    key_id: &utc_day,         // <-- decoupled
})
```

In `read_decrypted`:

```rust
let dek = self.key_manager.dek_for(&row.key_id)?;
let aad = format!("{}|{}|{}|{}", row.business_day, row.event_type, row.aggregate_id, row.key_id);
let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
```

- [ ] **Step 2: AppState wiring**

Add `pub key_manager: Arc<KeyManager>`. Construct in `lib.rs` once, before `EventService`. Pass `Arc::clone(&key_manager)` into `EventService::new`. Same `Arc` is reused by the rotation scheduler in Task 5.

- [ ] **Step 3: existing event_service tests**

Update the test rig to construct a KeyManager and pass it. The `aad_tamper_business_day_fails_decrypt` test's note already foresaw this: now that `business_day` and `key_id` are independent, mutating `business_day` on a row should produce a GCM auth failure (not a NotFound), because the row's `key_id` still resolves to the same DEK. Update the assertion to match: `matches!(err, AppError::Crypto(_))`.

- [ ] **Step 4: cli.rs (eod-now subcommand) constructs KeyManager**

Same wiring change. EventService construction must use `KeyManager::new` instead of the old `master+kek` fields.

- [ ] **Step 5: cargo test services::event_service && cargo test --lib**

Expected: all event_service tests green. Expect compile errors in any other call site that referenced `EventService { master, kek, ... }` — fix by adding `key_manager`.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(events): EventService takes KeyManager; key_id = utc_day (decoupled from business_day)"
```

---

## Task 5: RotationScheduler tokio task

**Files:**
- Create: `apps/cashier/src-tauri/src/rotation/mod.rs`
- Create: `apps/cashier/src-tauri/src/rotation/scheduler.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (add `mod rotation;` and spawn)

- [ ] **Step 1: scheduler implementation**

```rust
// apps/cashier/src-tauri/src/rotation/scheduler.rs
use crate::app_state::AppState;
use crate::services::utc_day::next_utc_midnight_ms;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // Catch-up at startup
        match state.key_manager.rotate(state.clock.now_ms()) {
            Ok(r) => tracing::info!("key rotation startup: today={}, created={}, deleted={:?}", r.today, r.created_today, r.deleted),
            Err(e) => tracing::error!("key rotation startup failed: {e}"),
        }
        loop {
            let now = state.clock.now_ms();
            let next = next_utc_midnight_ms(now);
            let wait_ms = (next - now).max(1000);
            tracing::info!("key rotation sleeping {} ms until next UTC midnight", wait_ms);
            sleep(Duration::from_millis(wait_ms as u64)).await;
            match state.key_manager.rotate(state.clock.now_ms()) {
                Ok(r) => tracing::info!("key rotation: today={}, created={}, deleted={:?}", r.today, r.created_today, r.deleted),
                Err(e) => tracing::error!("key rotation failed: {e}"),
            }
        }
    });
}
```

```rust
// apps/cashier/src-tauri/src/rotation/mod.rs
pub mod scheduler;
pub use scheduler::spawn;
```

- [ ] **Step 2: spawn from lib.rs**

After `eod::scheduler::spawn(state_arc.clone())`, add `rotation::spawn(state_arc.clone());`.

- [ ] **Step 3: integration test for catch-up**

```rust
// apps/cashier/src-tauri/tests/rotation_integration.rs
#[tokio::test]
async fn rotation_startup_creates_today_and_prunes() {
    use chrono::TimeZone;
    let now_ms = chrono::Utc.with_ymd_and_hms(2026, 4, 28, 12, 0, 0).unwrap().timestamp_millis();
    let state = test_helpers::seed_app_state_with_clock(now_ms).await;
    // Pre-seed 5-day-old key
    state.key_manager.current_dek_at("2026-04-22", now_ms).unwrap();
    // Trigger one rotation cycle synchronously (don't wait on the spawned task)
    let report = state.key_manager.rotate(now_ms).unwrap();
    assert!(report.deleted.contains(&"2026-04-22".to_string()));
    assert!(report.created_today);
}
```

(Don't actually drive the tokio sleep loop — `rotate` itself is the unit under test; the spawn is a thin wrapper.)

- [ ] **Step 4: cargo test rotation**

Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(rotation): tokio scheduler — daily UTC-midnight rotate() + startup catch-up"
```

---

## Task 6: EOD pipeline — drop dek delete, add event-row delete

**Files:**
- Modify: `apps/cashier/src-tauri/src/eod/runner.rs`

- [ ] **Step 1: locate the existing transaction in `run_eod`**

Inside the `with_tx` block, today's sequence is roughly:
```
INSERT OR REPLACE INTO daily_report ...
DELETE FROM day_key WHERE business_day = ?      <-- REMOVE
DELETE FROM idempotency_keys WHERE created_at < ?
DELETE FROM token_denylist WHERE expires_at < ?
UPDATE eod_runs SET status='ok' ...
```

- [ ] **Step 2: replace with**

```sql
INSERT OR REPLACE INTO daily_report ...                           (unchanged)
-- crypto-shred is now the rotation service's job; EOD does not touch dek.
DELETE FROM event WHERE business_day = ?1                          -- NEW
DELETE FROM idempotency_keys WHERE created_at < ?
DELETE FROM token_denylist WHERE expires_at < ?
UPDATE eod_runs SET status='ok' ...
```

The `event` table is in `events.db`, NOT master.db. The runner currently does master-db deletes inside one transaction. Two transactions (one per DB) are required:
1. Master tx: daily_report upsert + idempotency/denylist prune + eod_runs update.
2. Events tx (separate connection): `DELETE FROM event WHERE business_day = ?1`.

Order: events delete FIRST (after report file is written, before master tx). Rationale: if events delete succeeds but master tx fails, we lose audit data — bad. So:

Actually invert: master tx FIRST, then events delete. If events delete fails after master commits, the daily_report row still says "ok" but rows linger. Acceptable: rows are re-deletable on next EOD or on manual sweep, and they're already encrypted with a key that will expire in ≤3 days.

Add a final `events.delete_for_business_day(day)` step after `with_tx(..., "ok")`, log error on failure but do not flip status to failed. Add a unit test asserting "after run_eod, event rows for that business_day are gone."

- [ ] **Step 3: add `EventStore::delete_for_business_day(day)`**

In `store/events.rs`:
```rust
pub fn delete_for_business_day(&self, business_day: &str) -> AppResult<usize> {
    self.with_writer(|conn| {
        let n = conn.execute("DELETE FROM event WHERE business_day = ?1", params![business_day])?;
        Ok(n)
    })
}
```

(Use whichever `with_writer` / `with_connection` helper the EventStore Backend exposes — File and Memory variants both need to work.)

- [ ] **Step 4: tests**

Update existing `run_marks_eod_runs_ok_writes_report_deletes_day_key` — rename to `run_marks_eod_runs_ok_writes_report_deletes_event_rows`. Replace the `!day_key_exists` assertion with `event_count_for(state, "2026-04-27") == 0`.

Add `run_does_not_touch_dek` test: pre-seed a dek row, run EOD, assert dek row still present (rotation service owns its lifecycle).

- [ ] **Step 5: cargo test eod**

Expected: all eod tests pass after the rename + assertion swap.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(eod): drop dek delete (rotation service owns it); add event-row delete at cutoff"
```

---

## Task 7: GET /admin/keys endpoint

**Files:**
- Modify: `apps/cashier/src-tauri/src/http/routes/admin.rs`
- Modify: `packages/shared/src/schemas/admin.ts`

- [ ] **Step 1: route**

```rust
async fn list_keys(State(state): State<Arc<AppState>>, claims: Claims) -> AppResult<Json<Vec<DekInfo>>> {
    require(Action::ViewKeys, claims.role(), Default::default())?;
    let m = state.master.lock().unwrap();
    let info = m.list_dek_days()?;
    Ok(Json(info))
}

// router:
.route("/admin/keys", get(list_keys))
```

- [ ] **Step 2: ACL — add `Action::ViewKeys`**

```rust
// action.rs
ViewKeys,

// policy.rs
ViewKeys => allow_at(Role::Owner),
```

- [ ] **Step 3: shared schema**

```ts
// packages/shared/src/schemas/admin.ts (append)
export const DekInfo = z.object({
  utc_day: z.string(),
  created_at: z.number().int(),
}).strict();
export type DekInfo = z.infer<typeof DekInfo>;

export const DekInfoList = z.array(DekInfo);
```

- [ ] **Step 4: integration tests**

```rust
#[tokio::test] async fn admin_keys_lists_current_dek_days() { /* … */ }
#[tokio::test] async fn admin_keys_forbidden_for_non_owner() { /* manager hits → 403 */ }
```

- [ ] **Step 5: cargo test admin_integration::admin_keys && pnpm --filter @lofi-pos/shared typecheck**

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(admin): GET /admin/keys (Owner-only) — DEK retention visibility"
```

---

## Task 8: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — should grow from 205 to ~220+
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] All workspace typechecks: shared, pos-ui, cashier, web, admin
- [ ] Manual smoke (optional): `cashier eod-now <yesterday>` runs without touching dek; check `dek` table shows today's row; `event` table has no rows for yesterday.

---

## Done

End state:
- `dek` table replaces `day_key`; keyed by UTC calendar day; 3-day retention via `KeyManager::rotate`.
- `RotationScheduler` tokio task runs at every UTC midnight; on app start, it catches up.
- `EventService` writes `key_id = utc_day(now)` (independent of `business_day`); decrypts via `KeyManager::dek_for` which surfaces `Crypto("key expired")` cleanly.
- EOD pipeline no longer touches dek rows; instead deletes `event` table rows for the closed `business_day` after master tx commits.
- `GET /admin/keys` (Owner-only) lists currently-held dek days for ops visibility.

Two independent guarantees:
- **Crypto:** events older than ~72h are unreadable regardless of EOD status.
- **Data:** events for a closed business day are off the events table immediately after EOD.
