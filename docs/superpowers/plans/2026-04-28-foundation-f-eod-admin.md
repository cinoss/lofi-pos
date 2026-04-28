# Foundation Plan F — EOD Pipeline + Reports + Admin UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the foundation. Stand up the End-of-Day pipeline (report build → crypto-shred yesterday's day_key → prune idempotency/denylist), wire `print(type, data)` stubs at write sites, and ship a separate Owner-only admin UI for CRUD over spots/staff/products/settings.

**Architecture:**
- EOD scheduler: tokio task started in `lib.rs`. Computes the next cutoff (configurable hour, tz-offset-aware) and sleeps; on app start, replays any business days strictly before the current business day that lack a `daily_report` row.
- Report = JSON dump of all orders/items/payments for that business day (with spot snapshots already captured in the events). Stored both as a `daily_report` row AND written to `<app_data_dir>/reports/YYYY-MM-DD.json`.
- After report is persisted, `day_key.business_day = yesterday` row is deleted → events for that day become permanently unreadable (crypto-shred).
- Print = synchronous `print(kind, payload_json)` to stdout from CommandService side-effects (after broadcast). Replaceable with a real printer router later without touching call sites.
- Admin app: new `apps/admin` Vite SPA (no PWA, no service worker, no Tauri). Served by the cashier's axum at `/admin/*` static mount. Same login, same token, but every admin endpoint requires `Role::Owner`.

**Builds on:** Plans A through E1d.

**Out of scope (future):**
- Real printer driver / printer router process
- Inventory delta report (recipe-driven), payment-method breakdown, void/comp log — only the orders dump for now
- Edits to historical (closed-day) data
- Multi-venue / multi-tenant settings

---

## File Structure

```
apps/cashier/src-tauri/src/
  print.rs                                # NEW — `print(kind, payload_json)` stdout stub
  eod/
    mod.rs                                # NEW — pub use builder + runner + scheduler
    builder.rs                            # NEW — assemble report JSON for a business day
    runner.rs                             # NEW — single transactional EOD run
    scheduler.rs                          # NEW — tokio task: next-cutoff + startup catch-up
    business_day.rs                       # NEW — cutoff/tz arithmetic helpers
  http/
    routes/
      admin.rs                            # NEW — spot/staff/product/settings CRUD
      reports.rs                          # NEW — list/get daily_report
    static_admin.rs                       # NEW — serve apps/admin/dist at /admin/*
  store/migrations/master/
    0005_eod_runs.sql                     # NEW — eod_runs (audit log of runs)
  acl/action.rs                           # MODIFIED — add SpotEdit
  acl/policy.rs                           # MODIFIED — SpotEdit = Owner
  services/command_service.rs             # MODIFIED — print(...) side effect after broadcast
  lib.rs                                  # MODIFIED — start scheduler, mount admin routes/static
  main.rs                                 # MODIFIED — `cashier eod-now` subcommand (manual)

packages/shared/src/schemas/
  admin.ts                                # NEW — Spot/Staff/Product CRUD payloads
  report.ts                               # NEW — DailyReport + OrderDump shapes

apps/admin/                               # NEW Vite SPA
  package.json
  tsconfig.json, tsconfig.app.json, tsconfig.node.json
  .gitignore
  index.html
  vite.config.ts
  src/
    main.tsx                              # provider tree (reuses pos-ui auth)
    App.tsx                               # route tree (Owner-only gate at root)
    index.css                             # @import "@lofi-pos/ui/globals.css"
    routes/
      spots.tsx                           # list + create + edit + delete
      staff.tsx
      products.tsx
      settings.tsx                        # form for setting table
      reports.tsx                         # list daily_report + view JSON
```

Decisions:
- **Admin is a separate Vite app**, mounted under `/admin/` by axum's `tower_http::services::ServeDir`. Defaults `--admin-dist <path>` to `<workspace>/apps/admin/dist` in dev; Tauri bundle includes it as a resource for prod.
- **Owner-only at every admin endpoint** (server-side). Frontend additionally hides UI for non-Owners.
- **EOD scheduler uses tokio + chrono.** The business day boundary is `cutoff_hour` in `business_day_tz_offset_seconds`. Day `D` covers `[D 11:00 local, D+1 11:00 local)`; the report for `D` is written at the START of `D+1`, i.e. at `D+1 11:00 local`.
- **Crypto-shred is unconditional after report write.** No "soft" mode. The `daily_report` row IS the durable record; after delete-key the events are unreadable on disk.
- **Reports written to disk AND db.** The on-disk JSON is for ops/export. The db row is for in-app browsing and proves the report was generated.
- **`eod_runs` table**: `business_day TEXT PRIMARY KEY, started_at INT, finished_at INT, status TEXT, error TEXT NULL`. Lets us see which days were processed and what failed.
- **`print(kind, payload_json)`** signature is `fn print(kind: &str, payload: &serde_json::Value)`. v1 emits `[print] kind=order_ticket spot=Room 3 items=[...]` to stdout. Easy to mock in tests; trivial to replace.

---

## Task 1: `print()` stub + wire at command-service side-effects

**Files:**
- Create: `apps/cashier/src-tauri/src/print.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (`mod print;`)
- Modify: `apps/cashier/src-tauri/src/services/command_service.rs`

- [ ] **Step 1: write the test first**

```rust
// apps/cashier/src-tauri/src/print.rs (top)
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn print_emits_kind_and_compact_json() {
        let v = json!({"spot": "Table 1", "items": [{"name":"Coke","qty":1}]});
        let line = format_print_line("order_ticket", &v);
        assert!(line.starts_with("[print] kind=order_ticket "));
        assert!(line.contains("\"spot\":\"Table 1\""));
        assert!(!line.contains('\n'));
    }
}
```

- [ ] **Step 2: implementation**

```rust
// apps/cashier/src-tauri/src/print.rs
use serde_json::Value;

pub fn print(kind: &str, payload: &Value) {
    println!("{}", format_print_line(kind, payload));
}

pub(crate) fn format_print_line(kind: &str, payload: &Value) -> String {
    // Compact, single-line, easy to grep.
    format!("[print] kind={kind} payload={}", payload)
}
```

- [ ] **Step 3: side-effect map in command_service**

After the existing `broadcast_tx.send(...)` block in `CommandService::execute`, add per-event print:

```rust
// inside execute(), after store.apply + broadcast
use crate::domain::event::DomainEvent;
match &event {
    DomainEvent::OrderPlaced { spot, items, .. } => {
        // route to a printer per item kitchen/bar/none — for now, one ticket
        let payload = serde_json::json!({
            "spot": spot.display_name(),
            "items": items,
        });
        crate::print::print("order_ticket", &payload);
    }
    DomainEvent::PaymentRecorded { session_id, amount_total, method, .. } => {
        let payload = serde_json::json!({
            "session_id": session_id,
            "amount": amount_total,
            "method": method,
        });
        crate::print::print("receipt", &payload);
    }
    DomainEvent::SessionClosed { session_id, .. } => {
        let payload = serde_json::json!({ "session_id": session_id });
        crate::print::print("session_closed", &payload);
    }
    _ => {}
}
```

(If `SpotRef::display_name` doesn't exist, add it: `match self { Room { name, .. } | Table { name, .. } => name.clone() }`.)

- [ ] **Step 4: add `mod print;` in lib.rs**

- [ ] **Step 5: cargo test print:: + cargo test command_service**

```bash
cd apps/cashier/src-tauri && cargo test print:: && cargo test command_service
```

Expected: existing command_service tests still pass; new print test passes. Stdout from prints will appear in test output — that's fine.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cashier): print(kind, payload) stub at command-service write sites"
```

---

## Task 2: business_day arithmetic helpers

**Files:**
- Create: `apps/cashier/src-tauri/src/eod/mod.rs`
- Create: `apps/cashier/src-tauri/src/eod/business_day.rs`

- [ ] **Step 1: tests first**

```rust
// apps/cashier/src-tauri/src/eod/business_day.rs
#[cfg(test)]
mod tests {
    use super::*;

    // tz offset = +7h (25200 sec). cutoff = 11.
    // 2026-04-28 10:59 local = business_day 2026-04-27
    // 2026-04-28 11:00 local = business_day 2026-04-28
    fn cfg() -> Cfg { Cfg { cutoff_hour: 11, tz_offset_seconds: 25200 } }

    #[test]
    fn before_cutoff_belongs_to_previous_day() {
        // 2026-04-28 03:59 UTC == 10:59 +07
        let ts = ts_utc(2026, 4, 28, 3, 59, 0);
        assert_eq!(business_day_for(ts, cfg()), "2026-04-27");
    }

    #[test]
    fn at_cutoff_starts_new_day() {
        let ts = ts_utc(2026, 4, 28, 4, 0, 0); // 11:00 +07
        assert_eq!(business_day_for(ts, cfg()), "2026-04-28");
    }

    #[test]
    fn next_cutoff_after_today() {
        // currently 2026-04-28 12:00 +07 (=05:00 UTC); next cutoff is 2026-04-29 11:00 +07 = 04:00 UTC
        let now = ts_utc(2026, 4, 28, 5, 0, 0);
        let next = next_cutoff_ms(now, cfg());
        assert_eq!(next, ts_utc(2026, 4, 29, 4, 0, 0));
    }

    #[test]
    fn next_cutoff_before_today() {
        // currently 2026-04-28 09:00 +07 (=02:00 UTC); next cutoff is today 2026-04-28 11:00 +07 = 04:00 UTC
        let now = ts_utc(2026, 4, 28, 2, 0, 0);
        let next = next_cutoff_ms(now, cfg());
        assert_eq!(next, ts_utc(2026, 4, 28, 4, 0, 0));
    }

    fn ts_utc(y: i32, m: u32, d: u32, h: u32, mi: u32, s: u32) -> i64 {
        chrono::Utc
            .with_ymd_and_hms(y, m, d, h, mi, s)
            .unwrap()
            .timestamp_millis()
    }
}
```

- [ ] **Step 2: implementation**

```rust
// apps/cashier/src-tauri/src/eod/business_day.rs
use chrono::{DateTime, Duration, FixedOffset, NaiveDate, TimeZone, Utc};

#[derive(Debug, Clone, Copy)]
pub struct Cfg {
    pub cutoff_hour: u32,
    pub tz_offset_seconds: i32,
}

/// Returns the business day (YYYY-MM-DD) the timestamp belongs to.
pub fn business_day_for(ts_ms: i64, cfg: Cfg) -> String {
    let local = local(ts_ms, cfg);
    let shifted = local - Duration::hours(cfg.cutoff_hour as i64);
    shifted.format("%Y-%m-%d").to_string()
}

/// The UTC ms of the next cutoff strictly greater than `now_ms`.
pub fn next_cutoff_ms(now_ms: i64, cfg: Cfg) -> i64 {
    let local = local(now_ms, cfg);
    let cutoff_today = local
        .date_naive()
        .and_hms_opt(cfg.cutoff_hour, 0, 0)
        .unwrap();
    let target_local = if local.naive_local() < cutoff_today {
        cutoff_today
    } else {
        cutoff_today + Duration::days(1)
    };
    let tz = FixedOffset::east_opt(cfg.tz_offset_seconds).unwrap();
    tz.from_local_datetime(&target_local)
        .unwrap()
        .with_timezone(&Utc)
        .timestamp_millis()
}

fn local(ts_ms: i64, cfg: Cfg) -> DateTime<FixedOffset> {
    let tz = FixedOffset::east_opt(cfg.tz_offset_seconds).unwrap();
    Utc.timestamp_millis_opt(ts_ms).unwrap().with_timezone(&tz)
}

/// All business days in `[from..to)` (exclusive end). Used for catch-up.
pub fn days_between(from: &str, to: &str) -> Vec<String> {
    let f = NaiveDate::parse_from_str(from, "%Y-%m-%d").unwrap();
    let t = NaiveDate::parse_from_str(to, "%Y-%m-%d").unwrap();
    let mut out = Vec::new();
    let mut d = f;
    while d < t {
        out.push(d.format("%Y-%m-%d").to_string());
        d = d.succ_opt().unwrap();
    }
    out
}
```

```rust
// apps/cashier/src-tauri/src/eod/mod.rs
pub mod business_day;
pub mod builder;
pub mod runner;
pub mod scheduler;

pub use business_day::{business_day_for, days_between, next_cutoff_ms, Cfg};
```

- [ ] **Step 3: add `chrono` if not already a dep**

```bash
cd apps/cashier/src-tauri && cargo add chrono --features clock,serde
```

- [ ] **Step 4: cargo test eod::business_day**

Expected: 4/4 pass.

- [ ] **Step 5: lib.rs adds `mod eod;`**

(Files referenced by `mod.rs` will be empty stubs — create with `// stub` for now to satisfy the module resolution. Each later task fills one in and replaces the stub.)

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(eod): business-day arithmetic (cutoff + tz-offset aware)"
```

---

## Task 3: eod_runs migration + report builder

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0005_eod_runs.sql`
- Create: `apps/cashier/src-tauri/src/eod/builder.rs`

- [ ] **Step 1: migration**

```sql
-- 0005_eod_runs.sql
CREATE TABLE eod_runs (
  business_day TEXT PRIMARY KEY,
  started_at   INTEGER NOT NULL,
  finished_at  INTEGER,
  status       TEXT NOT NULL CHECK (status IN ('running','ok','failed')),
  error        TEXT
);
```

- [ ] **Step 2: builder test**

```rust
// apps/cashier/src-tauri/src/eod/builder.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::{seed_app_state_with_clock, place_test_order, take_test_payment};

    #[tokio::test]
    async fn builder_dumps_orders_for_day() {
        let state = seed_app_state_with_clock(/* fixed clock */).await;
        place_test_order(&state, /* day1 ts */).await;
        take_test_payment(&state, /* day1 ts */).await;
        let report = build_report(&state, "2026-04-27").await.unwrap();
        assert_eq!(report.business_day, "2026-04-27");
        assert_eq!(report.orders.len(), 1);
        assert_eq!(report.payments.len(), 1);
    }

    #[tokio::test]
    async fn empty_day_yields_empty_report() {
        let state = seed_app_state_with_clock(/* fixed clock */).await;
        let report = build_report(&state, "2026-04-27").await.unwrap();
        assert_eq!(report.orders.len(), 0);
    }
}
```

(If `test_helpers::seed_app_state_with_clock` doesn't accept a clock argument yet, extend it: `pub async fn seed_app_state_with_clock(now_ms: i64) -> AppState` — the existing seed helper instantiates a frozen `Clock`.)

- [ ] **Step 3: builder implementation**

```rust
// apps/cashier/src-tauri/src/eod/builder.rs
use crate::app_state::AppState;
use crate::domain::event::DomainEvent;
use crate::error::AppResult;
use crate::eod::business_day::{business_day_for, Cfg};
use serde::Serialize;
use serde_json::Value;

#[derive(Debug, Serialize)]
pub struct Report {
    pub business_day: String,
    pub generated_at: i64,
    pub orders: Vec<Value>,    // raw event payloads
    pub payments: Vec<Value>,
    pub sessions: Vec<Value>,
}

pub async fn build_report(state: &AppState, business_day: &str) -> AppResult<Report> {
    let cfg = current_cfg(state)?;
    let mut orders = Vec::new();
    let mut payments = Vec::new();
    let mut sessions = Vec::new();

    // Stream events for the day from events.db (caller supplies a fn or we call store directly).
    // Filter by business_day_for(event.ts_ms, cfg) == business_day.
    state.events.with_connection(|conn| {
        let mut stmt = conn.prepare(
            "SELECT id, kind, aggregate_id, ts_ms, payload_json FROM events ORDER BY id ASC"
        )?;
        let rows = stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })?;
        for row in rows {
            let (_id, kind, _agg, ts, payload_json) = row?;
            if business_day_for(ts, cfg) != business_day { continue; }
            let v: Value = serde_json::from_str(&payload_json).unwrap_or(Value::Null);
            match kind.as_str() {
                "OrderPlaced" | "OrderItemCancelled" | "OrderItemReturned" => orders.push(v),
                "PaymentRecorded" => payments.push(v),
                "SessionOpened" | "SessionClosed" | "SessionMerged" => sessions.push(v),
                _ => {}
            }
        }
        Ok::<_, rusqlite::Error>(())
    })?;

    Ok(Report {
        business_day: business_day.to_string(),
        generated_at: state.clock.now_ms(),
        orders,
        payments,
        sessions,
    })
}

fn current_cfg(state: &AppState) -> AppResult<Cfg> {
    let s = state.settings.get_all()?;
    Ok(Cfg {
        cutoff_hour: s.business_day_cutoff_hour as u32,
        tz_offset_seconds: s.business_day_tz_offset_seconds,
    })
}
```

(If `events.with_connection` doesn't exist, the existing pool API is `events.pool.get()?`. Use whichever is in `store/events.rs` already. If `state.events` Backend variants differ, dispatch on both.)

- [ ] **Step 4: cargo test eod::builder**

Expected: both tests pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(eod): report builder + eod_runs audit table"
```

---

## Task 4: EOD runner (transactional: report + crypto-shred + prune)

**Files:**
- Create/replace: `apps/cashier/src-tauri/src/eod/runner.rs`

- [ ] **Step 1: tests first**

```rust
// apps/cashier/src-tauri/src/eod/runner.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_helpers::*;

    #[tokio::test]
    async fn run_marks_eod_runs_ok_writes_report_deletes_day_key() {
        let state = seed_app_state_with_clock(ts_for("2026-04-27 12:00 +07")).await;
        place_test_order(&state, ts_for("2026-04-27 14:00 +07")).await;
        // a key for 2026-04-27 should now exist (created on first event)
        assert!(day_key_exists(&state, "2026-04-27"));
        let result = run_eod(&state, "2026-04-27").await.unwrap();
        assert_eq!(result.status, "ok");
        // daily_report row exists
        assert!(daily_report_exists(&state, "2026-04-27"));
        // file exists at <data_dir>/reports/2026-04-27.json
        assert!(state.reports_dir.join("2026-04-27.json").exists());
        // day_key gone (crypto-shred)
        assert!(!day_key_exists(&state, "2026-04-27"));
        // eod_runs row marked ok
        assert_eq!(eod_runs_status(&state, "2026-04-27"), "ok");
    }

    #[tokio::test]
    async fn run_idempotent_second_call_is_noop() {
        let state = seed_app_state_with_clock(ts_for("2026-04-27 12:00 +07")).await;
        place_test_order(&state, ts_for("2026-04-27 14:00 +07")).await;
        run_eod(&state, "2026-04-27").await.unwrap();
        // second call: already-done, returns early without error
        let again = run_eod(&state, "2026-04-27").await.unwrap();
        assert_eq!(again.status, "ok");
    }

    #[tokio::test]
    async fn run_prunes_idempotency_keys_for_day() {
        // seed an idempotency key with ts < cutoff for that day
        let state = seed_app_state_with_clock(ts_for("2026-04-27 12:00 +07")).await;
        insert_idempotency(&state, "key1", ts_for("2026-04-27 13:00 +07")).await;
        run_eod(&state, "2026-04-27").await.unwrap();
        assert!(!idempotency_exists(&state, "key1"));
    }
}
```

- [ ] **Step 2: implementation**

```rust
// apps/cashier/src-tauri/src/eod/runner.rs
use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use crate::eod::builder::build_report;
use rusqlite::params;
use std::fs;

#[derive(Debug)]
pub struct RunResult {
    pub business_day: String,
    pub status: &'static str,
}

pub async fn run_eod(state: &AppState, business_day: &str) -> AppResult<RunResult> {
    // Already done? (idempotent)
    let already: Option<String> = {
        let conn = state.master.lock().unwrap();
        conn.query_row(
            "SELECT status FROM eod_runs WHERE business_day = ?1",
            params![business_day],
            |r| r.get::<_, String>(0),
        )
        .ok()
    };
    if matches!(already.as_deref(), Some("ok")) {
        return Ok(RunResult { business_day: business_day.to_string(), status: "ok" });
    }

    // Mark running
    let now = state.clock.now_ms();
    {
        let conn = state.master.lock().unwrap();
        conn.execute(
            "INSERT OR REPLACE INTO eod_runs(business_day, started_at, status) VALUES (?1, ?2, 'running')",
            params![business_day, now],
        )?;
    }

    // Build report
    let report = match build_report(state, business_day).await {
        Ok(r) => r,
        Err(e) => {
            mark_failed(state, business_day, &e.to_string())?;
            return Err(e);
        }
    };

    // Persist: file
    fs::create_dir_all(&state.reports_dir).map_err(|e| AppError::Internal(e.to_string()))?;
    let path = state.reports_dir.join(format!("{business_day}.json"));
    fs::write(&path, serde_json::to_vec_pretty(&report).unwrap())
        .map_err(|e| AppError::Internal(e.to_string()))?;

    // Persist: db row (daily_report) + crypto-shred + prune, atomic
    let order_summary = serde_json::to_string(&report).unwrap();
    {
        let mut conn = state.master.lock().unwrap();
        let tx = conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO daily_report(business_day, generated_at, order_summary_json, inventory_summary_json) \
             VALUES (?1, ?2, ?3, '{}')",
            params![business_day, now, order_summary],
        )?;
        // crypto-shred: drop the day key
        tx.execute("DELETE FROM day_key WHERE business_day = ?1", params![business_day])?;
        // prune idempotency_keys created during/before that day's cutoff window
        // (use a generous bound: anything strictly before today's cutoff is safe)
        tx.execute(
            "DELETE FROM idempotency_keys WHERE created_at < ?1",
            params![now - 24 * 3600 * 1000],
        )?;
        tx.execute(
            "DELETE FROM token_denylist WHERE expires_at < ?1",
            params![now],
        )?;
        tx.execute(
            "UPDATE eod_runs SET finished_at = ?1, status = 'ok', error = NULL WHERE business_day = ?2",
            params![state.clock.now_ms(), business_day],
        )?;
        tx.commit()?;
    }

    // Drop in-memory aggregates whose events all belong to closed days. (v1: just clear nothing — warm_up
    // re-replays from disk on restart and skips events whose key is gone.) Out of scope.

    Ok(RunResult { business_day: business_day.to_string(), status: "ok" })
}

fn mark_failed(state: &AppState, day: &str, err: &str) -> AppResult<()> {
    let conn = state.master.lock().unwrap();
    conn.execute(
        "UPDATE eod_runs SET finished_at = ?1, status = 'failed', error = ?2 WHERE business_day = ?3",
        params![state.clock.now_ms(), err, day],
    )?;
    Ok(())
}
```

- [ ] **Step 3: AppState gains `reports_dir: PathBuf`**

In `app_state.rs`:
```rust
pub struct AppState {
    // ... existing fields
    pub reports_dir: std::path::PathBuf,
}
```

In `lib.rs` setup, after computing `app_data_dir`:
```rust
let reports_dir = app_data_dir.join("reports");
```
Pass into AppState.

- [ ] **Step 4: cargo test eod::runner**

Expected: 3/3 pass.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(eod): runner — write report, crypto-shred day_key, prune"
```

---

## Task 5: EOD scheduler (tokio task) + startup catch-up + manual subcommand

**Files:**
- Create/replace: `apps/cashier/src-tauri/src/eod/scheduler.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: `apps/cashier/src-tauri/src/main.rs`

- [ ] **Step 1: scheduler implementation**

```rust
// apps/cashier/src-tauri/src/eod/scheduler.rs
use crate::app_state::AppState;
use crate::eod::business_day::{business_day_for, days_between, next_cutoff_ms, Cfg};
use crate::eod::runner::run_eod;
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        // 1) catch-up
        if let Err(e) = catch_up(&state).await {
            tracing::error!("eod catch-up failed: {e}");
        }
        // 2) loop next cutoff
        loop {
            let cfg = match current_cfg(&state) {
                Ok(c) => c,
                Err(e) => {
                    tracing::error!("eod cfg read failed: {e}");
                    sleep(Duration::from_secs(60)).await;
                    continue;
                }
            };
            let now = state.clock.now_ms();
            let next = next_cutoff_ms(now, cfg);
            let wait_ms = (next - now).max(1000);
            tracing::info!("eod scheduler sleeping {} ms until next cutoff", wait_ms);
            sleep(Duration::from_millis(wait_ms as u64)).await;
            // Right after cutoff, the just-closed business day == business_day_for(now-1ms)
            let now2 = state.clock.now_ms();
            let cfg2 = current_cfg(&state).unwrap_or(cfg);
            let just_closed = business_day_for(now2 - 1000, cfg2);
            if let Err(e) = run_eod(&state, &just_closed).await {
                tracing::error!("eod run for {just_closed} failed: {e}");
            }
        }
    });
}

pub async fn catch_up(state: &AppState) -> crate::error::AppResult<()> {
    let cfg = current_cfg(state)?;
    let today = business_day_for(state.clock.now_ms(), cfg);
    // earliest unprocessed day = MIN(business_day) in events not in eod_runs(status='ok'),
    // or just iterate days strictly < today and run any not yet ok.
    let conn = state.master.lock().unwrap();
    let earliest: Option<String> = conn
        .query_row(
            "SELECT MIN(business_day) FROM day_key WHERE business_day < ?1",
            rusqlite::params![&today],
            |r| r.get::<_, Option<String>>(0),
        )
        .unwrap_or(None);
    drop(conn);
    let Some(start) = earliest else { return Ok(()); };
    for day in days_between(&start, &today) {
        let already = {
            let c = state.master.lock().unwrap();
            c.query_row(
                "SELECT status FROM eod_runs WHERE business_day = ?1",
                rusqlite::params![&day],
                |r| r.get::<_, String>(0),
            ).ok()
        };
        if matches!(already.as_deref(), Some("ok")) { continue; }
        if let Err(e) = run_eod(state, &day).await {
            tracing::error!("catch-up failed for {day}: {e}");
        }
    }
    Ok(())
}

fn current_cfg(state: &AppState) -> crate::error::AppResult<Cfg> {
    let s = state.settings.get_all()?;
    Ok(Cfg {
        cutoff_hour: s.business_day_cutoff_hour as u32,
        tz_offset_seconds: s.business_day_tz_offset_seconds,
    })
}
```

- [ ] **Step 2: spawn from lib.rs after AppState built**

```rust
// in lib.rs setup
let state_arc = Arc::new(state);
crate::eod::scheduler::spawn(state_arc.clone());
// also use state_arc for axum router
```

- [ ] **Step 3: `cashier eod-now [day]` subcommand**

In `main.rs`, before delegating to `cashier_lib::run()`:
```rust
let args: Vec<String> = std::env::args().collect();
if args.get(1).map(|s| s.as_str()) == Some("eod-now") {
    cashier_lib::cli::run_eod_now(args.get(2).cloned()).expect("eod-now");
    return;
}
```

Add `cli` module exposing `run_eod_now(day: Option<String>)` that opens master+events, builds an AppState shell (no axum, no scheduler), and invokes `run_eod`. Default day = yesterday.

- [ ] **Step 4: scheduler test (logical, no real time)**

Skip real timer testing in unit tests — `run_eod` is already tested. For the scheduler add one integration test that calls `catch_up` directly:

```rust
#[tokio::test]
async fn catch_up_processes_old_unprocessed_days() {
    let state = seed_app_state_with_clock(ts_for("2026-04-30 12:00 +07")).await;
    // seed events on 2026-04-27 and 2026-04-28
    place_test_order(&state, ts_for("2026-04-27 14:00 +07")).await;
    place_test_order(&state, ts_for("2026-04-28 14:00 +07")).await;
    catch_up(&state).await.unwrap();
    assert_eq!(eod_runs_status(&state, "2026-04-27"), "ok");
    assert_eq!(eod_runs_status(&state, "2026-04-28"), "ok");
}
```

- [ ] **Step 5: cargo test eod**

Expected: all eod tests pass.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(eod): scheduler (tokio) + startup catch-up + cashier eod-now CLI"
```

---

## Task 6: Admin CRUD HTTP endpoints + ACL

**Files:**
- Modify: `apps/cashier/src-tauri/src/acl/action.rs`
- Modify: `apps/cashier/src-tauri/src/acl/policy.rs`
- Create: `apps/cashier/src-tauri/src/http/routes/admin.rs`
- Create: `apps/cashier/src-tauri/src/http/routes/reports.rs`
- Modify: `apps/cashier/src-tauri/src/http/server.rs` (mount routes)
- Create: `packages/shared/src/schemas/admin.ts`
- Create: `packages/shared/src/schemas/report.ts`

- [ ] **Step 1: ACL — add SpotEdit, mark Owner**

```rust
// action.rs add variant: SpotEdit
// policy.rs match arm: SpotEdit => allow_at(Role::Owner),
```

(`EditMenu` already exists for products; `EditStaff`, `EditSettings` already exist.)

- [ ] **Step 2: shared zod schemas**

```ts
// packages/shared/src/schemas/admin.ts
import { z } from "zod";

export const SpotInput = z.object({
  name: z.string().min(1),
  kind: z.enum(["room", "table"]),
  hourly_rate: z.number().int().nullable(),
  parent_id: z.number().int().nullable(),
}).strict();
export type SpotInput = z.infer<typeof SpotInput>;

export const StaffInput = z.object({
  name: z.string().min(1),
  pin: z.string().min(6),
  role: z.enum(["staff","cashier","manager","owner"]),
  team: z.string().nullable(),
}).strict();
export type StaffInput = z.infer<typeof StaffInput>;

export const StaffUpdate = StaffInput.partial().strict();
export type StaffUpdate = z.infer<typeof StaffUpdate>;

export const ProductInput = z.object({
  name: z.string().min(1),
  price: z.number().int().nonnegative(),
  route: z.enum(["kitchen","bar","none"]),
  kind: z.enum(["item","recipe","time"]),
}).strict();
export type ProductInput = z.infer<typeof ProductInput>;

export const SettingsUpdate = z.object({
  business_day_cutoff_hour: z.number().int().min(0).max(23).optional(),
  business_day_tz_offset_seconds: z.number().int().optional(),
  discount_threshold_pct: z.number().int().optional(),
  cancel_grace_minutes: z.number().int().optional(),
  idle_lock_minutes: z.number().int().optional(),
}).strict();
export type SettingsUpdate = z.infer<typeof SettingsUpdate>;
```

```ts
// packages/shared/src/schemas/report.ts
import { z } from "zod";

export const DailyReport = z.object({
  business_day: z.string(),
  generated_at: z.number().int(),
  order_summary_json: z.string(),  // raw JSON string
}).strict();
export type DailyReport = z.infer<typeof DailyReport>;

export const DailyReportList = z.array(DailyReport);
```

Add to `packages/shared/src/index.ts`:
```ts
export * from "./schemas/admin";
export * from "./schemas/report";
```

- [ ] **Step 3: admin route handlers**

```rust
// http/routes/admin.rs (sketch)
// All handlers: ACL check first (Owner-only via existing EditMenu/EditStaff/EditSettings/SpotEdit),
// then SQL against master, return Json.
//
// Endpoints:
//   GET    /admin/spots          -> Vec<Spot>
//   POST   /admin/spots          -> Spot     (Action: SpotEdit)
//   PUT    /admin/spots/:id
//   DELETE /admin/spots/:id
//   GET    /admin/staff
//   POST   /admin/staff                       (Action: EditStaff; hash PIN with argon2)
//   PUT    /admin/staff/:id
//   DELETE /admin/staff/:id
//   GET    /admin/products
//   POST   /admin/products                    (Action: EditMenu)
//   PUT    /admin/products/:id
//   DELETE /admin/products/:id
//   GET    /admin/settings
//   PUT    /admin/settings                    (Action: EditSettings)
```

Tests required: per resource, one happy-path and one ACL-forbidden test (e.g., Cashier hits POST /admin/spots → 403).

(For PIN hashing on staff create, reuse `crate::auth::pin::hash_pin`. Min length already enforced by `MIN_PIN_LENGTH=6`.)

- [ ] **Step 4: reports route handlers**

```rust
// http/routes/reports.rs
// GET /admin/reports                 -> Vec<DailyReport summary> (id+generated_at)
// GET /admin/reports/:business_day   -> full DailyReport row
// All Owner-only. (Could be ViewLiveReports = Manager, but reports are admin scope.)
// Add Action::ViewReports if you want a separate variant; for now reuse RunEod (Owner).
```

(Decision: gate behind a new `Action::ViewReports = Manager`. Manager can view reports.)

- [ ] **Step 5: mount in server.rs**

```rust
.nest("/admin", admin::router(state.clone()))
.nest("/admin/reports", reports::router(state.clone()))
```

(Both apply the existing `auth_layer` so unauthenticated callers get 401 before ACL even runs.)

- [ ] **Step 6: cargo test http::routes::admin && cargo test http::routes::reports**

Expected: all pass. Existing 177 tests still green.

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(http): admin CRUD (spots/staff/products/settings) + reports endpoints"
```

---

## Task 7: Serve `apps/admin/dist` static at `/admin/*`

**Files:**
- Create: `apps/cashier/src-tauri/src/http/static_admin.rs`
- Modify: `apps/cashier/src-tauri/src/http/server.rs`
- Modify: `apps/cashier/src-tauri/Cargo.toml` (`tower-http = { features = ["fs"] }`)

- [ ] **Step 1: static handler**

```rust
// http/static_admin.rs
use axum::Router;
use tower_http::services::{ServeDir, ServeFile};
use std::path::PathBuf;

pub fn router(admin_dist: PathBuf) -> Router {
    let index = admin_dist.join("index.html");
    Router::new()
        .nest_service("/", ServeDir::new(admin_dist).not_found_service(ServeFile::new(index)))
}
```

(SPA fallback: any unknown path returns index.html so client-side routing works.)

- [ ] **Step 2: mount under `/ui/admin` (NOT `/admin`)**

The HTTP API endpoints already live at `/admin/*` from Task 6. Static files go at `/ui/admin/*` so they don't collide:

```rust
.nest_service("/ui/admin", static_admin::router(state.admin_dist.clone()))
```

(Adjust admin app vite `base: "/ui/admin/"`. Update base URL handling in admin app accordingly.)

- [ ] **Step 3: AppState gets `admin_dist: PathBuf`**

Default in `lib.rs`:
- Dev: `<workspace>/apps/admin/dist` (env override `LOFI_ADMIN_DIST`)
- Prod (Tauri bundle): resolve via `app_handle.path_resolver().resource_dir()` + `"admin"`

If the dir doesn't exist, mount returns 404 — log a warning at startup.

- [ ] **Step 4: smoke test (optional)**

```rust
#[tokio::test]
async fn ui_admin_root_returns_index_html() {
    let state = ...with_temp_admin_dist(b"<html>hi</html>");
    let res = get(&state, "/ui/admin/").await;
    assert_eq!(res.status, 200);
    assert!(res.body.contains("<html>"));
}
```

(Skip if too fiddly; this is a static-file passthrough.)

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(http): serve apps/admin/dist at /ui/admin/* (SPA fallback)"
```

---

## Task 8: Scaffold `apps/admin` Vite SPA

**Files:**
- Create: `apps/admin/{package.json,tsconfig.json,tsconfig.app.json,tsconfig.node.json,.gitignore,index.html,vite.config.ts}`
- Create: `apps/admin/src/{main.tsx,App.tsx,index.css}`
- Create: `apps/admin/src/routes/{spots,staff,products,settings,reports}.tsx`

- [ ] **Step 1: package.json**

```json
{
  "name": "@lofi-pos/admin",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "typecheck": "tsc -b"
  },
  "dependencies": {
    "@lofi-pos/pos-ui": "workspace:*",
    "@lofi-pos/shared": "workspace:*",
    "@lofi-pos/ui": "workspace:*",
    "@tanstack/react-query": "^5.59.0",
    "react": "^19.2.4",
    "react-dom": "^19.2.4",
    "react-router-dom": "^6.27.0",
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "@tailwindcss/vite": "^4.1.18",
    "@types/react": "^19.2.10",
    "@types/react-dom": "^19.2.3",
    "@vitejs/plugin-react": "^5.1.1",
    "typescript": "5.9.3",
    "vite": "^7.3.2"
  }
}
```

- [ ] **Step 2: tsconfig + .gitignore (cashier pattern — emitDeclarationOnly + .tsbuild-node + ignore vite.config.{js,d.ts})**

(Copy exactly the structure already present in `apps/cashier`.)

- [ ] **Step 3: vite.config.ts**

```ts
import path from "path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

export default defineConfig({
  base: "/ui/admin/",
  plugins: [react(), tailwindcss()],
  resolve: { alias: { "@": path.resolve(__dirname, "./src") } },
});
```

- [ ] **Step 4: index.html + index.css**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>LoFi POS — Admin</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

```css
@import "@lofi-pos/ui/globals.css";
```

- [ ] **Step 5: main.tsx — provider tree (reuses pos-ui)**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { ApiClient } from "@lofi-pos/shared";
import {
  ApiClientProvider, AuthProvider, SettingsProvider,
  getStoredToken, attachWS as attachWSFactory,
} from "@lofi-pos/pos-ui";
import "./index.css";
import App from "./App";

const API_BASE = import.meta.env.VITE_API_BASE ?? window.location.origin;
const WS_BASE = API_BASE.replace(/^http/, "ws");

const apiClient = new ApiClient({ baseUrl: API_BASE, getToken: getStoredToken });
const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 1000, refetchOnWindowFocus: false } },
});

const attachWS = (qc: QueryClient) =>
  attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ApiClientProvider client={apiClient}>
        <AuthProvider client={apiClient} attachWS={attachWS}>
          <SettingsProvider client={apiClient}>
            <BrowserRouter basename="/ui/admin"><App /></BrowserRouter>
          </SettingsProvider>
        </AuthProvider>
      </ApiClientProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 6: App.tsx — Owner-only gate + routes**

```tsx
import { Navigate, Route, Routes, Link, Outlet } from "react-router-dom";
import { useAuth, LoginRoute, LockRoute } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { SpotsRoute } from "./routes/spots";
import { StaffRoute } from "./routes/staff";
import { ProductsRoute } from "./routes/products";
import { SettingsRoute } from "./routes/settings";
import { ReportsRoute } from "./routes/reports";

function AdminShell() {
  const { claims, logout } = useAuth();
  if (claims?.role !== "owner") {
    return <div className="p-8 text-red-600">Owner role required.</div>;
  }
  return (
    <div className="min-h-screen flex flex-col">
      <header className="flex items-center justify-between border-b bg-white px-6 py-3">
        <Link to="/" className="text-xl font-semibold">LoFi POS — Admin</Link>
        <nav className="flex items-center gap-4 text-sm">
          <Link to="/spots">Spots</Link>
          <Link to="/staff">Staff</Link>
          <Link to="/products">Products</Link>
          <Link to="/settings">Settings</Link>
          <Link to="/reports">Reports</Link>
          <span className="text-gray-500">{claims.role} · staff #{claims.staff_id}</span>
          <Button size="sm" variant="ghost" onClick={() => void logout()}>Logout</Button>
        </nav>
      </header>
      <main className="flex-1 bg-gray-50 p-6"><Outlet /></main>
    </div>
  );
}

export default function App() {
  const { isAuthenticated, isLocked, token } = useAuth();
  if (isLocked || (token && !isAuthenticated)) {
    return <Routes><Route path="*" element={<LockRoute />} /></Routes>;
  }
  if (!isAuthenticated) {
    return <Routes><Route path="*" element={<LoginRoute />} /></Routes>;
  }
  return (
    <Routes>
      <Route element={<AdminShell />}>
        <Route path="/spots" element={<SpotsRoute />} />
        <Route path="/staff" element={<StaffRoute />} />
        <Route path="/products" element={<ProductsRoute />} />
        <Route path="/settings" element={<SettingsRoute />} />
        <Route path="/reports" element={<ReportsRoute />} />
        <Route path="*" element={<Navigate to="/spots" replace />} />
      </Route>
    </Routes>
  );
}
```

- [ ] **Step 7: routes — list/create/update/delete forms**

Each route uses `useApiClient()` + `useQuery`/`useMutation` against the `/admin/...` endpoints. Use the zod schemas from `@lofi-pos/shared` (`SpotInput`, `StaffInput`, etc.) for validation. Keep the UI plain: a table + a "+" button that opens a Dialog with the form, edit by clicking the row, delete with a confirm.

(For the Dialog, reuse the existing shadcn primitives in `@lofi-pos/ui` — there's already a `Dialog` component bundled with the shadcn install used by the cashier.)

- [ ] **Step 8: pnpm install + typecheck + build**

```bash
pnpm install
pnpm --filter @lofi-pos/admin typecheck
pnpm --filter @lofi-pos/admin build
```

- [ ] **Step 9: Commit**

```bash
git add -A
git commit -m "feat(admin): scaffold @lofi-pos/admin SPA — spots/staff/products/settings/reports"
```

---

## Task 9: Wire admin dist into Tauri bundle resources

**Files:**
- Modify: `apps/cashier/src-tauri/tauri.conf.json` (add `apps/admin/dist` as a resource)
- Modify: `apps/cashier/package.json` (build script chains: `pnpm --filter @lofi-pos/admin build && tauri build`)
- Modify: `apps/cashier/src-tauri/src/lib.rs` (resolve admin_dist from resource_dir at runtime)

- [ ] **Step 1: tauri.conf.json `bundle.resources` adds `"../../admin/dist/**"`**

(Path is relative to `src-tauri`. Confirm the actual key — Tauri 2 uses `bundle.resources` as a glob array.)

- [ ] **Step 2: cashier package.json build script**

```json
"build": "pnpm --filter @lofi-pos/admin build && tsc -b && vite build",
"tauri:build": "pnpm --filter @lofi-pos/admin build && tauri build"
```

- [ ] **Step 3: lib.rs resolves admin_dist**

```rust
let admin_dist = if let Ok(p) = std::env::var("LOFI_ADMIN_DIST") {
    PathBuf::from(p)
} else if let Some(res_dir) = app.path().resource_dir().ok() {
    res_dir.join("admin").join("dist")
} else {
    workspace_root.join("apps/admin/dist")
};
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "build(cashier): bundle apps/admin/dist as Tauri resource; resolve at runtime"
```

---

## Task 10: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — count grows from 177 to ~190+ (print + business_day + builder + runner + scheduler + admin/reports)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `pnpm --filter @lofi-pos/shared typecheck` clean
- [ ] `pnpm --filter @lofi-pos/pos-ui typecheck` clean
- [ ] `pnpm --filter @lofi-pos/cashier typecheck` clean
- [ ] `pnpm --filter @lofi-pos/web typecheck` clean
- [ ] `pnpm --filter @lofi-pos/admin typecheck && pnpm --filter @lofi-pos/admin build` clean
- [ ] Manual smoke: `pnpm --filter @lofi-pos/cashier tauri dev`, log in as Owner, navigate to `http://localhost:7878/ui/admin/`, edit a setting, verify it shows up in `/settings` API and on next cashier login refresh.
- [ ] Manual smoke: `cashier eod-now 2026-04-27` (or fixture day) — verify `daily_report` row exists, `reports/2026-04-27.json` written, `day_key` row for that day gone.

---

## Done

End state:
- `print(kind, payload_json)` fires from CommandService for OrderPlaced, PaymentRecorded, SessionClosed → stdout. Easy to swap for a real printer router.
- EOD scheduler runs daily at `business_day_cutoff_hour`; on app start, missed days are processed first.
- Each EOD run: builds JSON report (orders + payments + sessions for the day), writes both to `daily_report` row and `reports/YYYY-MM-DD.json`, deletes that day's `day_key` (crypto-shred → events become unreadable), prunes idempotency_keys + token_denylist.
- `cashier eod-now [day]` subcommand for manual runs.
- `/admin/*` HTTP endpoints: full CRUD for spots/staff/products/settings, list+get for daily_report. Owner-only (Manager for reports).
- `apps/admin` SPA: separate Vite app, served by axum at `/ui/admin/`, bundled as Tauri resource. Reuses pos-ui auth.

Foundation complete. Next milestones live outside the foundation series.
