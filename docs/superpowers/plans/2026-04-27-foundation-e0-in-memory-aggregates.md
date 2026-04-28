# Foundation Plan E0 — In-Memory Aggregate Store

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Refactor the cashier from "decrypt-and-fold-on-every-read" to a write-through in-memory aggregate store. End state: `load_session` and `load_order` return from `DashMap` in microseconds; validation reads memory; encryption/disk only on write and on warm-up. Bonus: the merge-bill semantic (target session inherits source orders) drops out for free.

**Architecture:** New `AggregateStore` with `DashMap<id, State>` for sessions/orders/payments + idempotency cache. A single `apply(event)` function mutates memory; same function used at write-time AND warm-up. `CommandService::execute` becomes: validate against memory → encrypt + append to events.db → apply to memory → cache result. Tauri startup runs `AggregateStore::warm_up` after KEK + Master + EventStore are open.

**Tech Stack:** No new deps (`dashmap` already added in Plan D).

**Spec:** `docs/superpowers/specs/2026-04-27-in-memory-aggregates-design.md`. Read that for the full design rationale; this plan is the executable conversion.

**Builds on:** Plan A + B + C + D. Per the design addendum, this is Plan E0 (refactor); Plan E1 will add HTTP/axum + tablet client.

**Out of scope (Plan E1):**
- axum HTTP+WS server
- packages/shared TS client
- apps/web tablet PWA
- `idle_lock_minutes` enforcement
- Override audit-trail schema (override_staff column on event)

**Out of scope (Plan F):**
- EOD pipeline + reports + print stubs (will integrate with the new in-memory eviction path)

---

## File Structure

```
apps/cashier/src-tauri/src/
  store/
    aggregate_store.rs                  # NEW — DashMap-backed runtime state
    events.rs                           # MODIFIED — add list_live_aggregate_ids
    master.rs                           # MODIFIED — add list_active_business_days, list_idempotency_for_day
  domain/
    session.rs                          # MODIFIED — SessionState gains order_ids: Vec<String>
    order.rs                            # MODIFIED — OrderState (no schema change but Serialize already there)
    payment.rs                          # unchanged
    apply.rs                            # NEW — single apply(store, event, ctx) function
  services/
    command_service.rs                  # MODIFIED — execute() mutates memory + persists
    validation.rs                       # MODIFIED — takes &AggregateStore
    event_service.rs                    # MODIFIED — write() only; read_decrypted moves to warm_up
  app_state.rs                          # MODIFIED — AppState owns AggregateStore
  lib.rs                                # MODIFIED — call warm_up on startup
tests/
  aggregate_store_integration.rs        # NEW — apply() per event type, warm-up replay, merge bill
```

Key decisions:
- **`apply` lives in `domain/apply.rs`**, not on `AggregateStore`. Pure function over `&AggregateStore + DomainEvent`. Easier to test, no method-on-DashMap awkwardness.
- **`SessionState.order_ids: Vec<String>`** added (reverse index). Maintained by `apply`.
- **Existing `domain::*::fold` functions stay** but get rewritten to delegate to `apply` (start with empty store, apply each event, return projection). Eliminates fold-vs-apply drift.
- **`EventService::read_decrypted` stays in `event_service.rs`** but is only called from `AggregateStore::warm_up`. Existing integration tests (`projection_integration.rs`) use it during warm-up assertions.

---

## Task 1: `SessionState.order_ids` field + apply skeleton

**Files:**
- Modify: `apps/cashier/src-tauri/src/domain/session.rs`
- Create: `apps/cashier/src-tauri/src/domain/apply.rs`
- Modify: `apps/cashier/src-tauri/src/domain/mod.rs`

- [ ] **Step 1: Add `order_ids` to SessionState**

In `domain/session.rs`:
```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub status: SessionStatus,
    pub room_id: Option<i64>,
    pub table_id: Option<i64>,
    pub opened_by: i64,
    pub customer_label: Option<String>,
    pub team: Option<String>,
    /// Order ids placed under this session, including any inherited from
    /// merged sources. Maintained by `domain::apply::apply`.
    #[serde(default)]
    pub order_ids: Vec<String>,
}
```

Update the existing `fold` impl in `session.rs` to push `order_ids` from `OrderPlaced` events whose `session_id` matches:
```rust
DomainEvent::OrderPlaced { session_id, order_id, .. } => {
    if let Some(s) = state.as_mut() {
        if session_id == &s.session_id {
            s.order_ids.push(order_id.clone());
        }
    }
}
```

(Add this case to the existing match in `fold`; don't break other branches.)

Also update SessionMerged to push absorbed order_ids — but `fold` only sees this aggregate's events. For now, leave `fold` as-is; the merge-bill behavior will be correct under the new `apply` path. Add a note.

Update existing tests that construct `SessionState` literals — add `order_ids: vec![]` to each (likely 4-6 sites).

- [ ] **Step 2: Create `domain/apply.rs` skeleton**

```rust
//! `apply(store, event, ctx)` — single-source-of-truth state transition.
//! Used by both `CommandService::execute` (write-time mutation) and
//! `AggregateStore::warm_up` (replay on startup). Eliminates fold-vs-apply drift.

use crate::domain::event::DomainEvent;
use crate::error::AppResult;
use crate::store::aggregate_store::AggregateStore;

pub struct ApplyCtx<'a> {
    pub aggregate_id: &'a str,
}

pub fn apply(store: &AggregateStore, event: &DomainEvent, ctx: ApplyCtx<'_>) -> AppResult<()> {
    // Implemented in Task 4.
    let _ = (store, event, ctx);
    Ok(())
}
```

(Won't compile yet — `AggregateStore` is Task 2.)

- [ ] **Step 3: Wire `apply` into domain/mod.rs**

```rust
pub mod apply;
pub mod event;
pub mod order;
pub mod payment;
pub mod session;
```

(Don't try to compile yet; Task 2 adds `AggregateStore`.)

- [ ] **Step 4: Verify session tests still compile**

Run: `cd apps/cashier/src-tauri && cargo check --lib --tests` (expect failure on `apply.rs::AggregateStore` reference).

Expected error: `unresolved import crate::store::aggregate_store`. That's OK — Task 2 fixes it.

If session.rs tests have `SessionState { ... order_ids: vec![] }` everywhere, those should compile.

- [ ] **Step 5: Commit (this commit will leave a temporary build break — Task 2 unblocks)**

Actually — better practice: don't commit a build break. Combine Tasks 1+2 into one commit. Or stub `AggregateStore` first, then add `order_ids`.

**Revised plan:** skip committing here. Move directly to Task 2; commit them together as one logical change.

---

## Task 2: `AggregateStore` skeleton

**Files:**
- Create: `apps/cashier/src-tauri/src/store/aggregate_store.rs`
- Modify: `apps/cashier/src-tauri/src/store/mod.rs`

- [ ] **Step 1: Create `aggregate_store.rs`**

```rust
//! Runtime in-memory aggregate state. Disk events.db is the durable log
//! and crash-recovery source; this struct holds the live projection that
//! commands read and mutate.
//!
//! Concurrency: `DashMap`'s shard locks let distinct aggregates proceed
//! without contention. The `agg_lock` (`KeyMutex<String>`) in
//! `CommandService::execute` serializes validate-then-apply for one
//! aggregate id, preventing TOCTOU between reads and writes. Idempotency
//! cache is mirrored to `master.idempotency_key` for restart durability.

use crate::domain::order::OrderState;
use crate::domain::payment::PaymentState;
use crate::domain::session::SessionState;
use dashmap::DashMap;

pub struct AggregateStore {
    pub sessions: DashMap<String, SessionState>,
    pub orders:   DashMap<String, OrderState>,
    pub payments: DashMap<String, PaymentState>,
    /// Idempotency cache (key → result_json). Mirrored to master.db for restart.
    pub idem:     DashMap<String, String>,
}

impl AggregateStore {
    pub fn new() -> Self {
        Self {
            sessions: DashMap::new(),
            orders:   DashMap::new(),
            payments: DashMap::new(),
            idem:     DashMap::new(),
        }
    }
}

impl Default for AggregateStore {
    fn default() -> Self { Self::new() }
}
```

- [ ] **Step 2: Wire into `store/mod.rs`**

```rust
pub mod aggregate_store;
pub mod events;
pub mod master;
pub mod migrations;
```

- [ ] **Step 3: cargo check**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: compiles. `apply.rs`'s reference to `AggregateStore` now resolves; the body is still a no-op stub.

- [ ] **Step 4: Run prior tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 133 still pass. (The new `order_ids` field has `#[serde(default)]` so prior serialized projections deserialize; literal constructions in tests have been updated.)

- [ ] **Step 5: Commit**

```bash
git add -u apps/cashier/src-tauri/src
git commit -m "feat(cashier): AggregateStore + apply() skeleton + SessionState.order_ids

Empty AggregateStore (DashMaps for sessions/orders/payments + idem cache).
domain::apply::apply is a no-op stub; Task 4 fills it. SessionState
gains order_ids reverse index (#[serde(default)] for back-compat)."
```

---

## Task 3: Master + EventStore queries needed for warm-up

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/master.rs`
- Modify: `apps/cashier/src-tauri/src/store/events.rs`

- [ ] **Step 1: `Master::list_active_business_days`**

Append to `master.rs`:
```rust
impl Master {
    /// Business days that have a wrapped DEK row — i.e., days for which
    /// events may still be encrypted on disk. Used by warm-up to derive
    /// every DEK in advance.
    pub fn list_active_business_days(&self) -> AppResult<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT business_day FROM day_key ORDER BY business_day ASC")?;
        let days = stmt.query_map([], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(days)
    }

    /// All idempotency cache rows for a given business day. Used by warm-up
    /// to repopulate the in-memory cache after restart.
    pub fn list_idempotency_for_day(&self, business_day: &str) -> AppResult<Vec<(String, String)>> {
        // We don't currently store business_day on idempotency_key. Two options:
        //  (a) add business_day column + migration
        //  (b) load ALL idempotency rows on startup
        // Choosing (b) for now — at <10k rows/day this is a one-time cheap scan.
        // The `business_day` arg is accepted for future API stability.
        let _ = business_day;
        let mut stmt = self.conn.prepare(
            "SELECT key, result_json FROM idempotency_key")?;
        let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
```

Add 2 unit tests in `mod tests`:
```rust
#[test]
fn list_active_business_days_returns_day_key_dates() {
    let m = Master::open_in_memory().unwrap();
    m.put_day_key("2026-04-27", &[1]).unwrap();
    m.put_day_key("2026-04-28", &[2]).unwrap();
    let days = m.list_active_business_days().unwrap();
    assert_eq!(days, vec!["2026-04-27", "2026-04-28"]);
}

#[test]
fn list_idempotency_for_day_returns_all() {
    let m = Master::open_in_memory().unwrap();
    m.put_idempotency("k1", "cmd", "{\"a\":1}", 1).unwrap();
    m.put_idempotency("k2", "cmd", "{\"b\":2}", 2).unwrap();
    let rows = m.list_idempotency_for_day("2026-04-27").unwrap();
    assert_eq!(rows.len(), 2);
}
```

- [ ] **Step 2: `EventStore::list_live_aggregate_ids`**

In `events.rs`:
```rust
impl EventStore {
    /// Aggregate ids that exist in the log but have no terminal event yet.
    /// Used by warm-up to limit replay to live aggregates only — closed
    /// sessions, paid sessions, merged sources, split sources, returned
    /// orders, etc., do NOT need their state in memory.
    pub fn list_live_aggregate_ids(&self) -> AppResult<Vec<String>> {
        self.with_read(|conn| {
            let mut stmt = conn.prepare(
                "SELECT DISTINCT aggregate_id FROM event WHERE aggregate_id NOT IN (
                    SELECT aggregate_id FROM event
                    WHERE type IN ('SessionClosed','SessionMerged','SessionSplit')
                )")?;
            let ids = stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(ids)
        })
    }
}
```

(`with_read` is the helper added in Plan D's `Backend` enum refactor.)

Add 1 unit test:
```rust
#[test]
fn list_live_aggregate_ids_excludes_closed() {
    let s = EventStore::open_in_memory().unwrap();
    s.append(ev("2026-04-27", "live", "SessionOpened", 1)).unwrap();
    s.append(ev("2026-04-27", "closed", "SessionOpened", 2)).unwrap();
    s.append(ev("2026-04-27", "closed", "SessionClosed", 3)).unwrap();
    let live = s.list_live_aggregate_ids().unwrap();
    assert_eq!(live, vec!["live"]);
}
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 133 + 3 = 136 pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): warm-up support — list_active_business_days, list_live_aggregate_ids"
```

---

## Task 4: Implement `apply()` per event type

**Files:** Modify `apps/cashier/src-tauri/src/domain/apply.rs`

- [ ] **Step 1: Implement all 9 event branches**

Replace `apply.rs` body:
```rust
use crate::domain::event::DomainEvent;
use crate::domain::order::{OrderItem, OrderState};
use crate::domain::payment::PaymentState;
use crate::domain::session::{SessionState, SessionStatus};
use crate::error::AppResult;
use crate::store::aggregate_store::AggregateStore;

pub struct ApplyCtx<'a> {
    pub aggregate_id: &'a str,
}

pub fn apply(store: &AggregateStore, event: &DomainEvent, ctx: ApplyCtx<'_>) -> AppResult<()> {
    match event {
        DomainEvent::SessionOpened { room_id, table_id, opened_by, customer_label, team } => {
            store.sessions.insert(ctx.aggregate_id.to_string(), SessionState {
                session_id: ctx.aggregate_id.to_string(),
                status: SessionStatus::Open,
                room_id: *room_id, table_id: *table_id,
                opened_by: *opened_by,
                customer_label: customer_label.clone(),
                team: team.clone(),
                order_ids: Vec::new(),
            });
        }
        DomainEvent::SessionClosed { .. } => {
            if let Some(mut s) = store.sessions.get_mut(ctx.aggregate_id) {
                s.status = SessionStatus::Closed;
            }
        }
        DomainEvent::SessionTransferred { to_room_id, to_table_id, .. } => {
            if let Some(mut s) = store.sessions.get_mut(ctx.aggregate_id) {
                s.room_id = *to_room_id;
                s.table_id = *to_table_id;
            }
        }
        DomainEvent::SessionMerged { into_session, sources } => {
            // Each source's state is removed from active sessions; its order_ids
            // are absorbed into the target. Source's "Merged" status only matters
            // for warm-up's correctness (we don't need it in memory at runtime,
            // because removal IS the merge).
            let mut absorbed: Vec<String> = Vec::new();
            for src in sources {
                if let Some((_, src_state)) = store.sessions.remove(src) {
                    absorbed.extend(src_state.order_ids);
                }
            }
            if let Some(mut target) = store.sessions.get_mut(into_session) {
                target.order_ids.extend(absorbed);
            }
        }
        DomainEvent::SessionSplit { from_session, .. } => {
            if let Some(mut s) = store.sessions.get_mut(from_session) {
                s.status = SessionStatus::Split;
            }
            // New sessions are created by separate SessionOpened events
            // emitted by the split command (caller's responsibility).
        }
        DomainEvent::OrderPlaced { session_id, order_id, items } => {
            store.orders.insert(order_id.clone(), OrderState {
                order_id: order_id.clone(),
                session_id: session_id.clone(),
                items: items.iter().cloned().map(|spec| OrderItem {
                    spec, cancelled: false, returned_qty: 0,
                }).collect(),
            });
            if let Some(mut s) = store.sessions.get_mut(session_id) {
                s.order_ids.push(order_id.clone());
            }
        }
        DomainEvent::OrderItemCancelled { order_id, item_index, .. } => {
            if let Some(mut o) = store.orders.get_mut(order_id) {
                if let Some(it) = o.items.get_mut(*item_index) {
                    it.cancelled = true;
                }
            }
        }
        DomainEvent::OrderItemReturned { order_id, item_index, qty, .. } => {
            if let Some(mut o) = store.orders.get_mut(order_id) {
                if let Some(it) = o.items.get_mut(*item_index) {
                    it.returned_qty += qty;
                }
            }
        }
        DomainEvent::PaymentTaken { session_id, subtotal, discount_pct, vat_pct, total, method } => {
            store.payments.insert(session_id.clone(), PaymentState {
                session_id: session_id.clone(),
                subtotal: *subtotal, discount_pct: *discount_pct,
                vat_pct: *vat_pct, total: *total,
                method: method.clone(),
            });
        }
    }
    Ok(())
}
```

- [ ] **Step 2: Add unit tests covering each branch**

In `apply.rs` add:
```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::event::OrderItemSpec;

    fn opened(opener: i64) -> DomainEvent {
        DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: opener,
            customer_label: Some("VIP".into()), team: None,
        }
    }

    #[test]
    fn opened_inserts_session() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a" }).unwrap();
        let r = s.sessions.get("a").unwrap();
        assert_eq!(r.status, SessionStatus::Open);
        assert!(r.order_ids.is_empty());
    }

    #[test]
    fn closed_marks_session() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a" }).unwrap();
        apply(&s, &DomainEvent::SessionClosed { closed_by: 1, reason: None },
              ApplyCtx { aggregate_id: "a" }).unwrap();
        assert_eq!(s.sessions.get("a").unwrap().status, SessionStatus::Closed);
    }

    #[test]
    fn order_placed_indexes_into_session() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "sess" }).unwrap();
        apply(&s, &DomainEvent::OrderPlaced {
            session_id: "sess".into(), order_id: "o1".into(),
            items: vec![OrderItemSpec { product_id: 1, qty: 1, unit_price: 100, note: None }],
        }, ApplyCtx { aggregate_id: "o1" }).unwrap();
        assert_eq!(s.sessions.get("sess").unwrap().order_ids, vec!["o1"]);
        assert!(s.orders.contains_key("o1"));
    }

    #[test]
    fn merge_absorbs_source_orders_and_removes_source() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "A" }).unwrap();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "B" }).unwrap();
        apply(&s, &DomainEvent::OrderPlaced {
            session_id: "A".into(), order_id: "oA".into(),
            items: vec![OrderItemSpec { product_id: 1, qty: 1, unit_price: 100, note: None }],
        }, ApplyCtx { aggregate_id: "oA" }).unwrap();
        apply(&s, &DomainEvent::OrderPlaced {
            session_id: "B".into(), order_id: "oB".into(),
            items: vec![OrderItemSpec { product_id: 2, qty: 1, unit_price: 200, note: None }],
        }, ApplyCtx { aggregate_id: "oB" }).unwrap();

        apply(&s, &DomainEvent::SessionMerged {
            into_session: "A".into(),
            sources: vec!["B".into()],
        }, ApplyCtx { aggregate_id: "A" }).unwrap();

        let a = s.sessions.get("A").unwrap();
        assert_eq!(a.order_ids, vec!["oA", "oB"]);
        assert!(s.sessions.get("B").is_none(), "source B removed from active sessions");
    }

    #[test]
    fn cancel_marks_order_item() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "sess" }).unwrap();
        apply(&s, &DomainEvent::OrderPlaced {
            session_id: "sess".into(), order_id: "o".into(),
            items: vec![
                OrderItemSpec { product_id: 1, qty: 1, unit_price: 100, note: None },
                OrderItemSpec { product_id: 2, qty: 1, unit_price: 200, note: None },
            ],
        }, ApplyCtx { aggregate_id: "o" }).unwrap();
        apply(&s, &DomainEvent::OrderItemCancelled {
            order_id: "o".into(), item_index: 1, reason: None,
        }, ApplyCtx { aggregate_id: "o" }).unwrap();
        let o = s.orders.get("o").unwrap();
        assert!(!o.items[0].cancelled);
        assert!(o.items[1].cancelled);
    }

    #[test]
    fn return_increments_returned_qty() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "sess" }).unwrap();
        apply(&s, &DomainEvent::OrderPlaced {
            session_id: "sess".into(), order_id: "o".into(),
            items: vec![OrderItemSpec { product_id: 1, qty: 5, unit_price: 100, note: None }],
        }, ApplyCtx { aggregate_id: "o" }).unwrap();
        apply(&s, &DomainEvent::OrderItemReturned {
            order_id: "o".into(), item_index: 0, qty: 2, reason: None,
        }, ApplyCtx { aggregate_id: "o" }).unwrap();
        assert_eq!(s.orders.get("o").unwrap().items[0].returned_qty, 2);
    }

    #[test]
    fn payment_taken_inserts_payment() {
        let s = AggregateStore::new();
        apply(&s, &DomainEvent::PaymentTaken {
            session_id: "sess".into(), subtotal: 100,
            discount_pct: 0, vat_pct: 8, total: 108, method: "cash".into(),
        }, ApplyCtx { aggregate_id: "pay" }).unwrap();
        let p = s.payments.get("sess").unwrap();
        assert_eq!(p.total, 108);
    }

    #[test]
    fn transfer_updates_room_and_table() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a" }).unwrap();
        apply(&s, &DomainEvent::SessionTransferred {
            from_room_id: Some(1), from_table_id: None,
            to_room_id: None, to_table_id: Some(7),
        }, ApplyCtx { aggregate_id: "a" }).unwrap();
        let r = s.sessions.get("a").unwrap();
        assert_eq!(r.room_id, None);
        assert_eq!(r.table_id, Some(7));
    }

    #[test]
    fn split_marks_source_split() {
        let s = AggregateStore::new();
        apply(&s, &opened(1), ApplyCtx { aggregate_id: "a" }).unwrap();
        apply(&s, &DomainEvent::SessionSplit {
            from_session: "a".into(),
            new_sessions: vec!["b".into(), "c".into()],
        }, ApplyCtx { aggregate_id: "a" }).unwrap();
        assert_eq!(s.sessions.get("a").unwrap().status, SessionStatus::Split);
    }
}
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib domain::apply`
Expected: 9 tests pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): apply() per event type with 9 unit tests

Single source of truth for state transitions, used by both write-time
mutation (Task 6) and warm-up replay (Task 5). Merge absorbs source
order_ids into target — the merge-bill semantic drops out for free."
```

---

## Task 5: `AggregateStore::warm_up`

**Files:** Modify `apps/cashier/src-tauri/src/store/aggregate_store.rs`

- [ ] **Step 1: Implement warm_up**

Append to `aggregate_store.rs`:
```rust
use crate::business_day::business_day_of;
use crate::crypto::{Dek, Kek};
use crate::domain::apply::{apply, ApplyCtx};
use crate::domain::event::DomainEvent;
use crate::error::{AppError, AppResult};
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::Clock;
use chrono::FixedOffset;
use std::collections::HashMap;

impl AggregateStore {
    /// Restore in-memory state from durable storage.
    ///
    /// 1. Unwrap every active business day's DEK so we can decrypt events.
    /// 2. Replay live aggregates (those without a terminal event) into apply().
    /// 3. Repopulate idempotency cache for the current business day.
    ///
    /// Costs O(events for live aggregates × decrypt). At POS scale this is
    /// well under 100ms even for a busy night with cross-midnight sessions.
    pub fn warm_up(
        &self,
        master: &Master,
        events: &EventStore,
        kek: &Kek,
        clock: &dyn Clock,
        tz: FixedOffset,
        cutoff_hour: u32,
    ) -> AppResult<WarmUpStats> {
        // Step 1: derive DEKs for every retained day.
        let active_days = master.list_active_business_days()?;
        let mut deks: HashMap<String, Dek> = HashMap::new();
        for day in &active_days {
            if let Some(wrapped) = master.get_day_key(day)? {
                deks.insert(day.clone(), kek.unwrap(&wrapped)?);
            }
        }

        // Step 2: replay live aggregates.
        let live = events.list_live_aggregate_ids()?;
        let mut events_replayed = 0usize;
        for agg in &live {
            for row in events.list_for_aggregate(agg)? {
                let dek = deks.get(&row.key_id)
                    .ok_or_else(|| AppError::Internal(
                        format!("warm_up: missing DEK for key_id {}", row.key_id)))?;
                let aad = format!("{}|{}|{}|{}",
                    row.business_day, row.event_type, row.aggregate_id, row.key_id);
                let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
                let ev: DomainEvent = serde_json::from_slice(&pt)
                    .map_err(|e| AppError::Internal(format!("warm_up deserialize: {e}")))?;
                apply(self, &ev, ApplyCtx { aggregate_id: &row.aggregate_id })?;
                events_replayed += 1;
            }
        }

        // Step 3: repopulate idempotency cache (current business day).
        let today = business_day_of(clock.now(), tz, cutoff_hour);
        let idem_rows = master.list_idempotency_for_day(&today)?;
        let idem_count = idem_rows.len();
        for (key, json) in idem_rows {
            self.idem.insert(key, json);
        }

        Ok(WarmUpStats {
            aggregates_replayed: live.len(),
            events_replayed,
            idem_rows_loaded: idem_count,
            active_days: active_days.len(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct WarmUpStats {
    pub aggregates_replayed: usize,
    pub events_replayed: usize,
    pub idem_rows_loaded: usize,
    pub active_days: usize,
}
```

- [ ] **Step 2: Integration test**

Create `apps/cashier/src-tauri/tests/aggregate_store_integration.rs`:
```rust
use cashier_lib::business_day::business_day_of;
use cashier_lib::crypto::{Dek, Kek};
use cashier_lib::domain::apply::{apply, ApplyCtx};
use cashier_lib::domain::event::{DomainEvent, OrderItemSpec};
use cashier_lib::services::day_key;
use cashier_lib::store::aggregate_store::AggregateStore;
use cashier_lib::store::events::{AppendEvent, EventStore};
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use cashier_lib::time::Clock;
use chrono::FixedOffset;

fn write_event(events: &EventStore, master: &Master, kek: &Kek,
               agg: &str, ev: &DomainEvent, day: &str, ts: i64,
               event_type: &str) {
    let dek = day_key::get_or_create(master, kek, day).unwrap();
    let aad = format!("{day}|{event_type}|{agg}|{day}");
    let payload = serde_json::to_vec(ev).unwrap();
    let blob = dek.encrypt(&payload, aad.as_bytes()).unwrap();
    events.append(AppendEvent {
        business_day: day, ts, event_type, aggregate_id: agg,
        actor_staff: Some(1), payload_enc: &blob, key_id: day,
    }).unwrap();
}

#[test]
fn warm_up_replays_live_session_with_orders() {
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let day = "2026-04-27";

    write_event(&events, &master, &kek, "sess1",
        &DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: 1,
            customer_label: Some("L".into()), team: None,
        }, day, 100, "SessionOpened");
    write_event(&events, &master, &kek, "ord1",
        &DomainEvent::OrderPlaced {
            session_id: "sess1".into(), order_id: "ord1".into(),
            items: vec![OrderItemSpec { product_id: 1, qty: 2, unit_price: 50, note: None }],
        }, day, 200, "OrderPlaced");

    let store = AggregateStore::new();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
    let stats = store.warm_up(&master, &events, &kek, &clock,
        FixedOffset::east_opt(7 * 3600).unwrap(), 11).unwrap();

    assert_eq!(stats.aggregates_replayed, 2);
    assert_eq!(stats.events_replayed, 2);
    let s = store.sessions.get("sess1").unwrap();
    assert_eq!(s.order_ids, vec!["ord1"]);
    assert!(store.orders.contains_key("ord1"));
}

#[test]
fn warm_up_skips_closed_sessions() {
    let master = Master::open_in_memory().unwrap();
    let events = EventStore::open_in_memory().unwrap();
    let kek = Kek::new_random();
    let day = "2026-04-27";

    write_event(&events, &master, &kek, "alive",
        &DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: 1,
            customer_label: None, team: None,
        }, day, 100, "SessionOpened");
    write_event(&events, &master, &kek, "dead",
        &DomainEvent::SessionOpened {
            room_id: Some(2), table_id: None, opened_by: 1,
            customer_label: None, team: None,
        }, day, 200, "SessionOpened");
    write_event(&events, &master, &kek, "dead",
        &DomainEvent::SessionClosed { closed_by: 1, reason: None },
        day, 300, "SessionClosed");

    let store = AggregateStore::new();
    let clock = MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0);
    store.warm_up(&master, &events, &kek, &clock,
        FixedOffset::east_opt(7 * 3600).unwrap(), 11).unwrap();

    assert!(store.sessions.contains_key("alive"));
    assert!(!store.sessions.contains_key("dead"));
}
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --test aggregate_store_integration`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): AggregateStore::warm_up replays live aggregates"
```

---

## Task 6: Refactor `CommandService` to mutate memory

**Files:**
- Modify: `apps/cashier/src-tauri/src/services/command_service.rs`
- Modify: `apps/cashier/src-tauri/src/services/validation.rs`
- Modify: `apps/cashier/src-tauri/src/app_state.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

This is the biggest single edit; treat carefully.

- [ ] **Step 1: Refactor `validation::validate` to take `&AggregateStore`**

Replace `validation.rs::validate` and `decrypt_aggregate`:
```rust
use crate::domain::event::DomainEvent;
use crate::domain::session::SessionStatus;
use crate::error::{AppError, AppResult};
use crate::store::aggregate_store::AggregateStore;

/// Pre-write invariant guard for domain events.
///
/// Reads in-memory state from `AggregateStore`. Race-free per aggregate
/// because `CommandService::execute` holds the per-aggregate `agg_lock`
/// for the duration of validate→write→apply.
pub fn validate(store: &AggregateStore, aggregate_id: &str, ev: &DomainEvent) -> AppResult<()> {
    match ev {
        DomainEvent::SessionClosed { .. } => {
            let s = store.sessions.get(aggregate_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session {aggregate_id} status {:?}, cannot close", s.status)));
            }
        }
        DomainEvent::SessionTransferred { .. } => {
            let s = store.sessions.get(aggregate_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session status {:?}, cannot transfer", s.status)));
            }
        }
        DomainEvent::SessionMerged { sources, .. } => {
            let target = store.sessions.get(aggregate_id)
                .ok_or_else(|| AppError::Validation("merge target not opened".into()))?;
            if target.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "merge target status {:?}", target.status)));
            }
            drop(target);
            for src in sources {
                let src_state = store.sessions.get(src)
                    .ok_or_else(|| AppError::Validation(
                        format!("merge source {src} not opened")))?;
                if src_state.status != SessionStatus::Open {
                    return Err(AppError::Conflict(format!(
                        "merge source {src} status {:?}", src_state.status)));
                }
            }
        }
        DomainEvent::SessionSplit { from_session, .. } => {
            let s = store.sessions.get(from_session)
                .ok_or_else(|| AppError::Validation("split source not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "split source status {:?}", s.status)));
            }
        }
        DomainEvent::OrderItemCancelled { order_id, item_index, .. } => {
            let o = store.orders.get(order_id)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() {
                return Err(AppError::Validation(format!(
                    "item_index {item_index} out of bounds (len {})", o.items.len())));
            }
            if o.items[*item_index].cancelled {
                return Err(AppError::Conflict("item already cancelled".into()));
            }
        }
        DomainEvent::OrderItemReturned { order_id, item_index, qty, .. } => {
            let o = store.orders.get(order_id)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() {
                return Err(AppError::Validation(format!(
                    "item_index {item_index} out of bounds (len {})", o.items.len())));
            }
            let it = &o.items[*item_index];
            let remaining = it.spec.qty - it.returned_qty;
            if *qty <= 0 || *qty > remaining {
                return Err(AppError::Validation(format!(
                    "return qty {qty} invalid (remaining {remaining})")));
            }
        }
        DomainEvent::PaymentTaken { session_id, .. } => {
            // CONTRACT: payment_cmd writes PaymentTaken with aggregate_id == session_id.
            if store.payments.contains_key(session_id) {
                return Err(AppError::Conflict("session already paid".into()));
            }
            let s = store.sessions.get(session_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session status {:?}, cannot take payment", s.status)));
            }
        }
        _ => {}
    }
    Ok(())
}
```

- [ ] **Step 2: Refactor `CommandService`**

Modify `command_service.rs`:

```rust
use crate::domain::apply::{apply, ApplyCtx};
use crate::store::aggregate_store::AggregateStore;
// ... existing imports ...

pub struct CommandService {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub event_service: EventService,
    pub clock: Arc<dyn Clock>,
    pub auth: Arc<AuthService>,
    pub idem_lock: Arc<KeyMutex<String>>,
    pub agg_lock: Arc<KeyMutex<String>>,
    pub store: Arc<AggregateStore>,           // NEW
}

impl CommandService {
    pub fn execute<T, F>(
        &self,
        actor: &TokenClaims,
        action: Action,
        ctx: policy::PolicyCtx,
        idempotency_key: &str,
        command_name: &str,
        aggregate_id: &str,
        event: DomainEvent,
        override_pin: Option<&str>,
        project: F,
    ) -> AppResult<(T, WriteOutcome)>
    where
        F: FnOnce(&Self) -> AppResult<T>,
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        // ACL
        let effective_actor_staff = match policy::check(action, actor.role, ctx) {
            policy::Decision::Allow => actor.staff_id,
            policy::Decision::Deny => return Err(AppError::Unauthorized),
            policy::Decision::OverrideRequired(min) => {
                let pin = override_pin.ok_or(AppError::OverrideRequired(min))?;
                self.auth.verify_pin_for_role(pin, min)?.id
            }
        };

        let _idem = self.idem_lock.lock(idempotency_key.to_string());

        // Cache check from MEMORY
        if let Some(cached) = self.store.idem.get(idempotency_key) {
            let v: T = serde_json::from_str(&cached)
                .map_err(|e| AppError::Internal(format!("cached parse: {e}")))?;
            return Ok((v, WriteOutcome::Cached));
        }

        let _agg = self.agg_lock.lock(aggregate_id.to_string());

        // Validate against memory
        validation::validate(&self.store, aggregate_id, &event)?;

        // Encrypt + persist event (uses event_service which encrypts via DEK + writes events.db)
        self.event_service.write(
            WriteCtx {
                aggregate_id,
                actor_staff: Some(effective_actor_staff),
                at: None,
            },
            &event,
        )?;

        // Mutate memory
        apply(&self.store, &event, ApplyCtx { aggregate_id })?;

        // Project from memory
        let projection = project(self)?;

        // Cache to memory + disk
        let now = self.clock.now().timestamp_millis();
        let json = serde_json::to_string(&projection)
            .map_err(|e| AppError::Internal(format!("project serialize: {e}")))?;
        self.store.idem.insert(idempotency_key.to_string(), json.clone());
        self.master.lock().unwrap()
            .put_idempotency(idempotency_key, command_name, &json, now)?;

        Ok((projection, WriteOutcome::Inserted))
    }

    pub fn load_session(&self, id: &str) -> AppResult<Option<crate::domain::session::SessionState>> {
        Ok(self.store.sessions.get(id).map(|r| r.clone()))
    }

    pub fn load_order(&self, id: &str) -> AppResult<Option<crate::domain::order::OrderState>> {
        Ok(self.store.orders.get(id).map(|r| r.clone()))
    }

    pub fn list_active_sessions(&self) -> AppResult<Vec<crate::domain::session::SessionState>> {
        Ok(self.store.sessions.iter()
            .filter(|r| r.value().status == crate::domain::session::SessionStatus::Open)
            .map(|r| r.value().clone())
            .collect())
    }
}
```

- [ ] **Step 3: Update `AppState` + `lib.rs`**

In `app_state.rs`:
```rust
pub struct AppState {
    pub kek: Arc<Kek>,
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub clock: Arc<dyn Clock>,
    pub auth: AuthService,
    pub commands: CommandService,
    pub store: Arc<AggregateStore>,    // NEW (also held inside commands)
}
```

In `lib.rs::run` setup:
```rust
let store = Arc::new(crate::store::aggregate_store::AggregateStore::new());

// Warm up from disk BEFORE managing AppState
let stats = store.warm_up(
    &master.lock().unwrap(), &events, &kek, &*clock, tz, cutoff_hour,
)?;
tracing::info!(?stats, "aggregate store warm-up complete");

let commands = services::command_service::CommandService {
    master: master.clone(),
    events: events.clone(),
    event_service,
    clock: clock.clone(),
    auth: Arc::new(auth.clone()),
    idem_lock: Arc::new(services::locking::KeyMutex::new()),
    agg_lock: Arc::new(services::locking::KeyMutex::new()),
    store: store.clone(),     // NEW
};

app.manage(app_state::AppState {
    kek, master, events, clock, auth, commands, store,
});
```

- [ ] **Step 4: Fix `validation_integration.rs`**

The 5 tests there call `validate(svc, events, agg_id, ev)`. Update to `validate(&store, agg_id, ev)` after first applying any setup events to `store` instead of writing them through EventService. Restructure each test:
```rust
fn rig() -> AggregateStore {
    AggregateStore::new()
}
// Then apply setup events directly via apply().
```

Or more pragmatic: keep using `EventService` for setup, then apply each setup event to the `store` to mirror state. Effectively:
```rust
fn rig() -> (AggregateStore, EventService, Arc<Mutex<Master>>, Arc<EventStore>) {
    // ... build everything ...
    (store, svc, master, events)
}

fn apply_setup(store: &AggregateStore, agg: &str, ev: &DomainEvent) {
    apply(store, ev, ApplyCtx { aggregate_id: agg }).unwrap();
}
```

Each test:
1. Build rig
2. Apply opening event to store via `apply` (no encryption/disk needed)
3. Call `validate(&store, agg, &candidate_event)` and assert.

- [ ] **Step 5: Update `commands_integration.rs`, `hardening_integration.rs`, `override_integration.rs`, `remaining_commands_integration.rs`**

Each test rig:
- Build `Arc<AggregateStore>`
- Pass into `CommandService::store`
- Call `warm_up` on a fresh in-memory store before tests run (no events yet → no-op but exercises the path)

OR: don't call warm_up in tests since no prior events exist. The store starts empty; commands populate it.

The tests' existing assertions (like `events.list_for_aggregate(...)`) still work — disk is still written.

Each test uses the new return shape and `state.commands.execute(..., None, ...)`.

- [ ] **Step 6: Fix `projection_integration.rs`**

This integration test currently builds events on disk, then reads + decrypts + folds via `EventService::read_decrypted`. After refactor:
- Path A (preserve): keep using `EventService::read_decrypted` since it's still public.
- Path B: switch to using `AggregateStore` + `apply` directly. Cleaner long-term.

Use Path A (minimal change) — the test is asserting end-to-end disk-then-decrypt-then-fold semantics, which warm-up uses. The test still validates the warm-up code path indirectly.

- [ ] **Step 7: Run all tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 136 + 9 (apply unit) + 2 (warm-up integration) = 147 pass.

- [ ] **Step 8: Run clippy + fmt**

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`

- [ ] **Step 9: Commit**

```bash
git add -u
git commit -m "refactor(cashier): CommandService mutates AggregateStore; reads from memory

Hot path now: validate against memory → encrypt + append → apply to memory
→ project from memory. Zero disk reads on the read side after warm-up.
load_session/load_order = dashmap.get(id).clone(). list_active_sessions
filters memory directly. Validation reads &AggregateStore."
```

---

## Task 7: Bill helper across merged sources

**Files:** Modify `apps/cashier/src-tauri/src/services/command_service.rs` (or new `bill.rs`)

- [ ] **Step 1: Add `compute_bill`**

```rust
impl CommandService {
    /// Sum live subtotal for a session, including orders inherited from any
    /// merged-in source sessions. After Plan E0's `apply(SessionMerged)`, the
    /// target session's `order_ids` already contain the absorbed ids.
    pub fn compute_bill(&self, session_id: &str) -> AppResult<i64> {
        let s = self.store.sessions.get(session_id)
            .ok_or(AppError::NotFound)?;
        let order_ids = s.order_ids.clone();
        drop(s);
        let mut total = 0;
        for oid in order_ids {
            if let Some(o) = self.store.orders.get(&oid) {
                total += o.live_subtotal();
            }
        }
        Ok(total)
    }
}
```

- [ ] **Step 2: Integration test for merged bill**

Append to `tests/aggregate_store_integration.rs`:
```rust
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::auth::AuthService;
use std::sync::{Arc, Mutex};

#[test]
fn merged_session_bill_includes_source_orders() {
    // Build the full stack
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let store = Arc::new(AggregateStore::new());
    let clock: Arc<MockClock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let event_service = EventService {
        master: master.clone(), events: events.clone(), kek: kek.clone(),
        clock: clock.clone(), cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService { master: master.clone(), clock: clock.clone(), signing_key: signing };

    // Apply events directly to memory (skip command pipeline for brevity)
    apply(&store, &DomainEvent::SessionOpened {
        room_id: Some(1), table_id: None, opened_by: 1,
        customer_label: None, team: None,
    }, ApplyCtx { aggregate_id: "A" }).unwrap();
    apply(&store, &DomainEvent::SessionOpened {
        room_id: Some(2), table_id: None, opened_by: 1,
        customer_label: None, team: None,
    }, ApplyCtx { aggregate_id: "B" }).unwrap();

    apply(&store, &DomainEvent::OrderPlaced {
        session_id: "A".into(), order_id: "oA".into(),
        items: vec![OrderItemSpec { product_id: 1, qty: 2, unit_price: 50_000, note: None }],
    }, ApplyCtx { aggregate_id: "oA" }).unwrap();
    apply(&store, &DomainEvent::OrderPlaced {
        session_id: "B".into(), order_id: "oB".into(),
        items: vec![OrderItemSpec { product_id: 2, qty: 1, unit_price: 200_000, note: None }],
    }, ApplyCtx { aggregate_id: "oB" }).unwrap();

    apply(&store, &DomainEvent::SessionMerged {
        into_session: "A".into(),
        sources: vec!["B".into()],
    }, ApplyCtx { aggregate_id: "A" }).unwrap();

    let cs = CommandService {
        master, events, event_service, clock,
        auth: Arc::new(auth),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
        store,
    };

    let bill = cs.compute_bill("A").unwrap();
    assert_eq!(bill, 2 * 50_000 + 1 * 200_000);
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test --test aggregate_store_integration
git add -u
git commit -m "feat(cashier): compute_bill across merged sources via order_ids index"
```

---

## Task 8: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — ~148 tests pass
- [ ] `cargo clippy --all-targets -- -D warnings` — clean
- [ ] `cargo fmt --check` — clean
- [ ] `pnpm --filter @tableorder/cashier typecheck` — clean
- [ ] If fmt commit needed: `git add -u && git commit -m "chore(cashier): cargo fmt"`

---

## Done

End state:
- `AggregateStore` holds runtime state in `DashMap`s (sessions, orders, payments, idempotency cache)
- `apply(store, event)` is the single source of truth for state transitions
- Hot path: validate-against-memory → encrypt+append → apply-to-memory → project-from-memory
- Zero disk reads on command read path after warm-up
- Merge sets up bill correctly via `order_ids` reverse index — no cross-aggregate fold needed
- Warm-up replays only live aggregates (~50ms for typical POS state)
- Idempotency cache restored on startup

Plan E1 picks up:
- axum HTTP+WS server bound to LAN (mirror command surface)
- packages/shared TS client (zod + typed fetch + WS hook)
- apps/web tablet PWA (PIN login → cookie → command calls)
- `idle_lock_minutes` enforcement
- override audit-trail schema (override_staff column)
- Settings cache on AppState

Plan F:
- EOD pipeline (fold closed sessions → reports → drop encrypted day rows + memory state)
- Print stub call sites
- Inventory delta computation
