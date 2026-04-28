# In-Memory Aggregate Store — Design Addendum

**Date:** 2026-04-27
**Augments:** `2026-04-27-foundation-design.md`
**Status:** Draft

## Purpose

Replace Plan D's "decrypt + replay on every read" model with a **write-through in-memory aggregate store**. Disk events become the durable log + crash-recovery source; runtime state lives in `DashMap`s; reads never touch disk.

This addresses the perf observation that current `load_session` / `load_order` re-decrypt and re-fold the entire aggregate stream on every command, AND it cleanly resolves the "merge bill" problem (source session orders should be visible from the merge target) by making projection state mutable instead of derived.

## Non-Goals

- Distributed consensus (single-process Tauri). No Raft, no quorum.
- Snapshot persistence to disk (events.db is enough; replay cost is fine for POS scale).
- Eviction beyond "drop closed sessions at EOD." No LRU.

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│ CommandService                                              │
│  execute(...) =                                             │
│    acl_check                                                │
│    idem_lock                                                │
│      cache check              ← memory (DashMap)            │
│      agg_lock                                               │
│        validate against memory state                        │
│        encrypt + events.append                              │
│        apply(event) → mutate memory state                   │
│        cache_put memory + master.db                         │
│      project from memory                                    │
└─────────────────────────────────────────────────────────────┘
            ▲                          │
            │                          │ write-through
            │                          ▼
┌─────────────────────────┐  ┌──────────────────────────────┐
│ AggregateStore (memory) │  │ EventStore + Master (disk)   │
│  sessions, orders,      │◄─│  events.db (encrypted log)   │
│  payments, idem cache   │  │  master.db (KEK + cache)     │
└─────────────────────────┘  └──────────────────────────────┘
            ▲
            │ replay on startup
            │
       warm_up()
```

## Data Structures

```rust
pub struct AggregateStore {
    sessions: DashMap<String, SessionState>,    // active + recently-closed (until EOD)
    orders:   DashMap<String, OrderState>,
    payments: DashMap<String, PaymentState>,
    idem:     DashMap<String, String>,           // memory mirror of idempotency_key.result_json
}
```

`SessionState`, `OrderState`, `PaymentState` already exist (Plan B). They become **mutable** at runtime — `DashMap::get_mut` returns `RefMut`. Locking is per-aggregate via the existing `agg_lock` from Plan D, so DashMap's per-shard locking is supplemental but not relied on for invariants.

## `apply(event)` — Single Source of Truth

The same `apply` function used at write-time AND at warm-up replay. Drift between projection-fold and apply is the biggest risk; eliminating two implementations eliminates the risk.

```rust
impl AggregateStore {
    pub fn apply(&self, ev: &DomainEvent, ctx: ApplyCtx) -> AppResult<()> {
        match ev {
            DomainEvent::SessionOpened { room_id, table_id, opened_by, customer_label, team } => {
                self.sessions.insert(ctx.aggregate_id.to_string(), SessionState {
                    session_id: ctx.aggregate_id.to_string(),
                    status: SessionStatus::Open,
                    room_id: *room_id, table_id: *table_id,
                    opened_by: *opened_by,
                    customer_label: customer_label.clone(),
                    team: team.clone(),
                    order_ids: Vec::new(),       // NEW field — replaces cross-aggregate fold
                });
            }
            DomainEvent::SessionClosed { .. } => {
                if let Some(mut s) = self.sessions.get_mut(ctx.aggregate_id) {
                    s.status = SessionStatus::Closed;
                }
            }
            DomainEvent::SessionTransferred { to_room_id, to_table_id, .. } => {
                if let Some(mut s) = self.sessions.get_mut(ctx.aggregate_id) {
                    s.room_id = *to_room_id;
                    s.table_id = *to_table_id;
                }
            }
            DomainEvent::SessionMerged { into_session, sources } => {
                // Target inherits all source order_ids.
                let mut absorbed: Vec<String> = Vec::new();
                for src in sources {
                    if let Some((_, src_state)) = self.sessions.remove(src) {
                        absorbed.extend(src_state.order_ids);
                        // Source session marked Merged in its own slot — but since we
                        // remove it from the map, "Merged" status only matters for
                        // event-replay correctness during warm-up. For runtime, removal
                        // from the active map IS the merge.
                        // (For audit, the disk events still record the merge.)
                    }
                }
                if let Some(mut target) = self.sessions.get_mut(into_session) {
                    target.order_ids.extend(absorbed);
                }
            }
            DomainEvent::SessionSplit { from_session, new_sessions } => {
                // Source becomes Split; new sessions are created as Open shells with
                // partition of order_ids handed to caller's command logic.
                if let Some(mut s) = self.sessions.get_mut(from_session) {
                    s.status = SessionStatus::Split;
                }
                // New session entries are written by separate SessionOpened events
                // emitted by the split command.
            }
            DomainEvent::OrderPlaced { session_id, order_id, items } => {
                self.orders.insert(order_id.clone(), OrderState {
                    order_id: order_id.clone(),
                    session_id: session_id.clone(),
                    items: items.iter().cloned().map(|spec| OrderItem {
                        spec, cancelled: false, returned_qty: 0,
                    }).collect(),
                });
                // Index back into session
                if let Some(mut s) = self.sessions.get_mut(session_id) {
                    s.order_ids.push(order_id.clone());
                }
            }
            DomainEvent::OrderItemCancelled { order_id, item_index, .. } => {
                if let Some(mut o) = self.orders.get_mut(order_id) {
                    if let Some(it) = o.items.get_mut(*item_index) {
                        it.cancelled = true;
                    }
                }
            }
            DomainEvent::OrderItemReturned { order_id, item_index, qty, .. } => {
                if let Some(mut o) = self.orders.get_mut(order_id) {
                    if let Some(it) = o.items.get_mut(*item_index) {
                        it.returned_qty += qty;
                    }
                }
            }
            DomainEvent::PaymentTaken { session_id, subtotal, discount_pct, vat_pct, total, method } => {
                self.payments.insert(session_id.clone(), PaymentState {
                    session_id: session_id.clone(),
                    subtotal: *subtotal, discount_pct: *discount_pct,
                    vat_pct: *vat_pct, total: *total,
                    method: method.clone(),
                });
            }
        }
        Ok(())
    }
}
```

**Note the new `SessionState.order_ids: Vec<String>` field.** This is the explicit reverse index that makes "compute bill across merged sources" trivial: target session has all order_ids; iterate and look each up.

The existing `domain::*::fold` functions are **kept** for one purpose: warm-up replay. They construct fresh state from a sequence; `apply` mutates existing state. To eliminate drift, refactor `fold` to repeatedly call `apply` on a new `AggregateStore` initialized empty — same code path.

## Validation Against Memory

`services::validation::validate` currently re-decrypts the aggregate from disk. New version reads from `AggregateStore`:

```rust
pub fn validate(store: &AggregateStore, aggregate_id: &str, ev: &DomainEvent) -> AppResult<()> {
    match ev {
        DomainEvent::SessionClosed { .. } => {
            let s = store.sessions.get(aggregate_id)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != SessionStatus::Open {
                return Err(AppError::Conflict(format!("status {:?}", s.status)));
            }
        }
        DomainEvent::OrderItemCancelled { order_id, item_index, .. } => {
            let o = store.orders.get(order_id)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() { return Err(AppError::Validation(...)); }
            if o.items[*item_index].cancelled { return Err(AppError::Conflict("already cancelled".into())); }
        }
        // ... same for Return, Payment, Transfer, Merge, Split ...
    }
    Ok(())
}
```

Zero crypto, zero disk. Same invariants.

## CommandService::execute (revised)

```rust
pub fn execute<T, F>(...) -> AppResult<(T, WriteOutcome)> {
    // ACL — unchanged
    let actor_id = match policy::check(...) { Allow → actor.staff_id, ... };

    let _idem = self.idem_lock.lock(idempotency_key.to_string());

    // Cache check from memory
    if let Some(cached) = self.store.idem.get(idempotency_key) {
        let v: T = serde_json::from_str(&cached)?;
        return Ok((v, WriteOutcome::Cached));
    }

    let _agg = self.agg_lock.lock(aggregate_id.to_string());

    // Validate memory state
    validation::validate(&self.store, aggregate_id, &event)?;

    // Encrypt + persist event
    let blob = encrypt(...);
    self.events.append(...)?;

    // Mutate memory
    self.store.apply(&event, ApplyCtx { aggregate_id })?;

    // Project from memory
    let projection = project(self)?;

    // Cache memory + disk
    let json = serde_json::to_string(&projection)?;
    self.store.idem.insert(idempotency_key.to_string(), json.clone());
    self.master.lock().unwrap().put_idempotency(idempotency_key, command_name, &json, now)?;

    Ok((projection, WriteOutcome::Inserted))
}

pub fn load_session(&self, id: &str) -> Option<SessionState> {
    self.store.sessions.get(id).map(|r| r.clone())
}
```

## Warm-up (Startup Recovery)

```rust
impl AggregateStore {
    pub fn warm_up(&self, events: &EventStore, master: &Master, kek: &Kek) -> AppResult<()> {
        // 1. Restore active business days' DEKs
        let active_days = master.list_active_business_days()?;  // NEW master query
        let mut deks: HashMap<String, Dek> = HashMap::new();
        for day in &active_days {
            if let Some(wrapped) = master.get_day_key(day)? {
                deks.insert(day.clone(), kek.unwrap(&wrapped)?);
            }
        }

        // 2. For each unfinished aggregate, replay its events into apply()
        let live_aggs = events.list_live_aggregate_ids()?;  // NEW: aggregates without terminal events
        for agg in live_aggs {
            for row in events.list_for_aggregate(&agg)? {
                let dek = deks.get(&row.key_id)
                    .ok_or_else(|| AppError::Internal("missing dek".into()))?;
                let aad = format!("{}|{}|{}|{}", row.business_day, row.event_type, row.aggregate_id, row.key_id);
                let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
                let ev: DomainEvent = serde_json::from_slice(&pt)?;
                self.apply(&ev, ApplyCtx { aggregate_id: &row.aggregate_id })?;
            }
        }

        // 3. Restore idempotency cache for current business day
        let today = business_day_of(now, tz, cutoff);
        for (key, json) in master.list_idempotency_for_day(&today)? {
            self.idem.insert(key, json);
        }

        Ok(())
    }
}
```

`events.list_live_aggregate_ids()` — needs implementing. SQL:
```sql
SELECT DISTINCT aggregate_id FROM event WHERE aggregate_id NOT IN (
    SELECT aggregate_id FROM event WHERE type IN ('SessionClosed','SessionMerged','SessionSplit')
)
```
(Approximation; will catch Open sessions and Open orders. Sufficient for warm-up — closed sessions/orders being absent from memory is fine.)

## Eviction

- **SessionClosed** → leave in memory until EOD (cashier may want to re-print bill)
- **EOD pipeline** (Plan F):
  1. Fold all closed sessions for that business day → reports
  2. Drop encrypted day rows from `events.db`
  3. Drop in-memory state for those closed sessions/orders
  4. Drop in-memory idempotency cache for that day
  5. Delete the wrapped DEK from `master.day_key` (crypto-shred completes)
- **Open-cross-midnight sessions** stay in memory through EOD; their events were tagged with the OPENING business day so the next day's DEK doesn't apply to them.

## Concurrency

- `agg_lock` (per-aggregate) protects validate-then-apply
- `idem_lock` (per-key) protects cache-check-then-write
- `DashMap`'s shard locks provide cheap concurrent access for distinct aggregates
- `apply` is fast (microseconds); holding `agg_lock` for the full pipeline is fine

## Migration from Plan D

What stays:
- `KeyMutex` (per-key + per-agg locks)
- `EventService::write` (encrypt + append)
- `bootstrap`, `crypto`, `keychain`, `auth`, `acl`, `policy`, `validation` shape (signature changes from `events: &EventStore` to `store: &AggregateStore`)
- `Master::put_idempotency` (still durable backing store)
- All Tauri command files (only the `project` closure changes — call `cs.load_*` which now hits memory)
- `EventStore::open`, `append`, `delete_day`, `vacuum`

What changes:
- `services::event_service::EventService::read_decrypted` — now ONLY used during warm-up. Move from EventService to AggregateStore::warm_up's body, or keep in EventService.
- `CommandService::load_session/load_order` — read from `AggregateStore` instead of decrypt+fold
- `validation::validate` — takes `&AggregateStore` instead of `(svc, events, agg_id)`
- `services::idempotency::run` — still used? Probably collapse into `CommandService::execute`'s inline pattern (already done in Plan C).
- `EventStore::list_for_aggregate` becomes warm-up-only API (still exposed for tests)

What disappears:
- The r2d2 read pool added in Plan D Task 3 — reads no longer hit `events.db` on hot path. Pool can shrink to size 1 (used only by warm-up + Plan F EOD fold) OR be removed entirely (single Mutex<Connection> for both warm-up reads and writer).
- `command_service::load_session` / `load_order`'s "list_for_aggregate then decrypt then fold" pattern — replaced with one `dashmap.get`

## Test Strategy

Unit tests on `AggregateStore::apply` (one per event type, asserts state after).

Integration test for warm-up:
1. Open session, place order, place another order, close one, restart with new AggregateStore
2. Replay → assert resulting state matches what the live process had

Property test: random sequence of events → apply each via `apply` → final state matches `domain::*::fold` of same sequence. Catches drift between the two paths.

The existing 133 tests cover the disk-side correctness; rewriting them to use AggregateStore is a mechanical sweep but worth scheduling.

## Open Decisions

1. **`fold` functions kept or removed?** Keep — they're the "construct fresh state from sequence" path used by `apply` itself (call `fold` then store result). OR remove and let `apply` BE the fold (start with empty `AggregateStore`, apply each event). Recommend the latter — single code path, no drift possible.

2. **Mutable state vs immutable + replace?** DashMap holds owned values; `get_mut` returns `RefMut`. Mutation in place is fine. Avoid Cow/Arc-swapping — adds complexity for zero gain in single-process.

3. **Idempotency cache TTL.** Memory cache for current business day; restored on warm-up from disk. EOD evicts both. (Already in design above.)

4. **Recovery on partial-write failure.** If `events.append` succeeds but power dies before `apply` runs: next startup's warm-up replays the event from disk, apply runs, state is correct. Self-healing.

5. **Recovery on cache mismatch.** If memory cache says X but disk says Y for an idempotency key: trust memory (it was written most recently to memory) — but they should never diverge since we write to both inside the agg_lock. Defensive: warm-up loads disk → memory; subsequent writes go memory + disk.

## What This Buys

| Metric | Before | After |
|---|---|---|
| Reads on hot path | ~3 SQL + N decrypts | 0 |
| Writes on hot path | 1 SQL + 1 encrypt | 1 SQL + 1 encrypt + memory mutation |
| Restart cost | ~0 | replay live aggregates (~50ms for 50 sessions) |
| Memory footprint | ~0 | ~1MB per 100 active sessions |
| Code paths to maintain | fold-on-read | apply-and-warm-up |
| Merge-bill semantic | requires cross-aggregate fold | trivial (re-point order_ids in memory) |
| Room/table status | derive on read | derive from sessions DashMap iteration |

## Migration Plan

Two PRs:

**Plan E0 — Refactor to in-memory aggregates** (~1.5 days, mechanical):
- Add `AggregateStore` + `apply`
- Refactor `validate` to take `&AggregateStore`
- Refactor `CommandService::execute` to mutate memory + persist
- Add `warm_up` + new `Master::list_active_business_days` + `EventStore::list_live_aggregate_ids` + `Master::list_idempotency_for_day`
- Tauri startup runs `warm_up` after AppState construction
- Update all 133 tests

**Plan E1 — HTTP/axum + tablet client + idle lock** (the original Plan E scope):
- Now operates entirely on the in-memory store; trivially fast
- Plan E handoff items (override audit, settings cache, snapshot cache) — snapshot cache becomes irrelevant, settings cache still useful

EOD pipeline (Plan F) gains a new step: drop in-memory state for closed aggregates after report write.
