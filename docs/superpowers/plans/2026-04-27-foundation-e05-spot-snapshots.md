# Foundation Plan E0.5 — Spot Unification + Event Snapshots

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make event payloads self-contained snapshots so historical reports reproduce regardless of master-data edits, and unify "room or table" into a single `Spot` concept (sessions are EITHER a room OR a table — never both, never neither).

**Architecture:** Collapse `room` + `table` master tables into one `spot` table with a `kind` discriminator. Replace `SessionState.{room_id, table_id}` with a single `Spot { kind, id, name, hourly_rate? }`. Snapshot `spot_name`/`hourly_rate` into `SessionOpened`/`SessionTransferred` events; snapshot `product_name`/`route`/`recipe_snapshot` (with `ingredient_name`) into `OrderItemSpec`; snapshot `actor_name` as a plaintext column on the event row.

**Tech Stack:** No new deps.

**Spec addendum:** `docs/superpowers/specs/2026-04-27-in-memory-aggregates-design.md` (relates to event-payload independence). Plan E0.5 is a pre-requisite for both Plan E1 (HTTP/tablet) and Plan F (EOD reports) — locks the event payload shape before HTTP exposes commands externally.

**Builds on:** Plan A through E0. Touches the deepest layer (event schema), so it's a focused refactor wave.

**Out of scope (Plan E1):**
- axum HTTP+WS, tablet, idle lock
- override_staff column on event (still plumbed only in payload via override_pin path)

**Out of scope (Plan F):**
- EOD pipeline + print stubs
- Inventory delta computation USING the new recipe snapshot

---

## File Structure

```
apps/cashier/src-tauri/src/
  store/
    migrations/master/
      0003_spot_unification.sql         # NEW — drop room/table, create spot, seed defaults if any
      0004_event_actor_name.sql         # NEW — ALTER TABLE event ADD COLUMN actor_name TEXT
                                        #       (events.db migration, not master)
    migrations/events/
      0002_actor_name.sql               # NEW
    master.rs                           # MODIFIED — Spot/SpotKind types; list_spots/get_spot/create_spot;
                                        #            drop Room/Table types and list_rooms/list_tables
    events.rs                           # MODIFIED — EventRow + AppendEvent gain actor_name field
  domain/
    spot.rs                             # NEW — SpotRef enum + SpotKind
    event.rs                            # MODIFIED — DomainEvent::SessionOpened/Transferred use SpotRef;
                                        #            OrderItemSpec gains product_name, route, recipe_snapshot
    session.rs                          # MODIFIED — SessionState.spot: SpotRef (replaces room_id/table_id)
    apply.rs                            # MODIFIED — use snapshot fields
    mod.rs                              # MODIFIED — declare spot
  services/
    event_service.rs                    # MODIFIED — WriteCtx gains actor_name; write() persists it
    validation.rs                       # MODIFIED — minimal (no logic change beyond shape)
    command_service.rs                  # MODIFIED — execute() takes actor_name, plumbs through
  commands/
    catalog_cmd.rs                      # MODIFIED — list_spots replaces list_rooms+list_tables
    session_cmd.rs                      # MODIFIED — open_session takes spot_id; resolves to SpotRef
    order_cmd.rs                        # MODIFIED — place_order takes items: [{product_id, qty, note}];
                                        #            resolves each to OrderItemSpec with snapshots
tests/                                  # ALL test files updated for new shapes
```

Decisions baked in:
- **One `spot` table** — collapses room + table. `kind: 'room' | 'table'`. `hourly_rate` only meaningful when kind=room (nullable for tables).
- **`SessionState.spot: SpotRef`** — type-safe one-of. No more `room_id?: ..., table_id?: ...` illegal-state pair.
- **Recipe snapshot lives inside `OrderItemSpec`** — every order item carries its own recipe at order-time. EOD inventory deltas use the snapshot, not current `recipe` table.
- **`actor_name` is a plaintext column on the event row** (NOT in encrypted payload). Reasoning: needed for audit display without decrypt; no PII concern (staff names are not customer data); simpler than payload changes.
- **No production data exists** — schema migrations drop tables freely. No data-preservation rewrites needed.

---

## Task 1: Schema migration — `spot` table + plaintext `actor_name`

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0003_spot_unification.sql`
- Create: `apps/cashier/src-tauri/src/store/migrations/events/0002_actor_name.sql`

- [ ] **Step 1: Master migration**

```sql
-- 0003_spot_unification.sql
DROP TABLE IF EXISTS "table";
DROP TABLE IF EXISTS room;

CREATE TABLE spot (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL,
  kind         TEXT NOT NULL CHECK (kind IN ('room','table')),
  hourly_rate  INTEGER,                                -- VND, only meaningful for kind='room'
  parent_id    INTEGER REFERENCES spot(id) ON DELETE SET NULL,
                                                       -- table inside a room (optional)
  status       TEXT NOT NULL DEFAULT 'idle',
  CHECK (kind = 'room' OR hourly_rate IS NULL)
);

CREATE INDEX idx_spot_kind ON spot(kind);
```

- [ ] **Step 2: Events migration**

```sql
-- 0002_actor_name.sql
ALTER TABLE event ADD COLUMN actor_name TEXT;
```

- [ ] **Step 3: Run migrations test**

Run: `cd apps/cashier/src-tauri && cargo test --test migrations_integration`
Expected: existing 4 tests still pass after the new migration applies. Update `expected_tables_exist_after_migration` to:
- replace `"room"` and `"table"` with `"spot"` in the table list
- (events test doesn't check column list, so no change there)

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/store/migrations apps/cashier/src-tauri/tests/migrations_integration.rs
git commit -m "feat(cashier): spot table replaces room+table; actor_name column on event"
```

---

## Task 2: `Spot` types + Master CRUD

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: Replace Room/Table with Spot**

In `master.rs`:
- Remove `pub struct Room { ... }` and `pub struct Table { ... }`
- Remove `list_rooms()` and `list_tables()`
- Add:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SpotKind { Room, Table }

impl SpotKind {
    pub fn as_str(self) -> &'static str {
        match self { SpotKind::Room => "room", SpotKind::Table => "table" }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s { "room" => Some(SpotKind::Room), "table" => Some(SpotKind::Table), _ => None }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Spot {
    pub id: i64,
    pub name: String,
    pub kind: SpotKind,
    pub hourly_rate: Option<i64>,    // present only when kind=Room
    pub parent_id: Option<i64>,      // table inside a room
    pub status: String,
}

impl Master {
    pub fn create_spot(&self, name: &str, kind: SpotKind,
        hourly_rate: Option<i64>, parent_id: Option<i64>) -> AppResult<i64>
    {
        if kind == SpotKind::Table && hourly_rate.is_some() {
            return Err(AppError::Validation("table cannot have hourly_rate".into()));
        }
        if kind == SpotKind::Room && hourly_rate.is_none() {
            return Err(AppError::Validation("room must have hourly_rate".into()));
        }
        self.conn.execute(
            "INSERT INTO spot(name, kind, hourly_rate, parent_id, status)
             VALUES(?1, ?2, ?3, ?4, 'idle')",
            params![name, kind.as_str(), hourly_rate, parent_id],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_spot(&self, id: i64) -> AppResult<Option<Spot>> {
        Ok(self.conn.query_row(
            "SELECT id, name, kind, hourly_rate, parent_id, status FROM spot WHERE id = ?1",
            params![id], row_to_spot,
        ).optional()?)
    }

    pub fn list_spots(&self) -> AppResult<Vec<Spot>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, kind, hourly_rate, parent_id, status FROM spot ORDER BY id ASC")?;
        let rows = stmt.query_map([], row_to_spot)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn row_to_spot(r: &rusqlite::Row<'_>) -> rusqlite::Result<Spot> {
    let kind_str: String = r.get(2)?;
    let kind = SpotKind::parse(&kind_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(2, rusqlite::types::Type::Text,
            format!("bad spot kind: {kind_str}").into())
    })?;
    Ok(Spot {
        id: r.get(0)?, name: r.get(1)?, kind,
        hourly_rate: r.get(3)?, parent_id: r.get(4)?, status: r.get(5)?,
    })
}
```

- [ ] **Step 2: Master tests**

Add 5 tests in `staff_tests` module (or new `spot_tests`):
- `create_room_with_rate_succeeds`
- `create_table_with_rate_rejected` (validation error)
- `create_room_without_rate_rejected`
- `get_spot_by_id`
- `list_spots_ordered`

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib store::master`
Expected: prior 10 + 5 new = 15.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): Spot/SpotKind types + Master CRUD"
```

---

## Task 3: Domain `SpotRef` + event payload changes

**Files:**
- Create: `apps/cashier/src-tauri/src/domain/spot.rs`
- Modify: `apps/cashier/src-tauri/src/domain/mod.rs`
- Modify: `apps/cashier/src-tauri/src/domain/event.rs`

- [ ] **Step 1: SpotRef**

Create `apps/cashier/src-tauri/src/domain/spot.rs`:
```rust
use serde::{Deserialize, Serialize};

/// Snapshot of a spot at session-open / transfer time. Self-contained — does
/// not depend on the live `spot` master row, so historical reports reproduce
/// regardless of subsequent renames or deletions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SpotRef {
    Room {
        id: i64,
        name: String,
        hourly_rate: i64,
    },
    Table {
        id: i64,
        name: String,
        room_id: Option<i64>,
        room_name: Option<String>,
    },
}

impl SpotRef {
    pub fn id(&self) -> i64 {
        match self { SpotRef::Room { id, .. } | SpotRef::Table { id, .. } => *id }
    }
    pub fn name(&self) -> &str {
        match self { SpotRef::Room { name, .. } | SpotRef::Table { name, .. } => name }
    }
    pub fn is_room(&self) -> bool { matches!(self, SpotRef::Room { .. }) }
    pub fn is_table(&self) -> bool { matches!(self, SpotRef::Table { .. }) }
}
```

Add `pub mod spot;` to `domain/mod.rs`.

- [ ] **Step 2: Update DomainEvent payloads**

In `domain/event.rs`:

```rust
use crate::domain::spot::SpotRef;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecipeIngredientSnapshot {
    pub ingredient_id: i64,
    pub ingredient_name: String,
    pub qty: f64,
    pub unit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Route { Kitchen, Bar, None }

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OrderItemSpec {
    pub product_id: i64,
    pub product_name: String,
    pub qty: i64,
    pub unit_price: i64,
    pub note: Option<String>,
    pub route: Route,
    pub recipe_snapshot: Vec<RecipeIngredientSnapshot>,
}
```

Update `SessionOpened` and `SessionTransferred` variants:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum DomainEvent {
    SessionOpened {
        spot: SpotRef,
        opened_by: i64,
        customer_label: Option<String>,
        team: Option<String>,
    },
    SessionClosed { /* unchanged */ closed_by: i64, reason: Option<String> },
    SessionTransferred {
        from: SpotRef,
        to: SpotRef,
    },
    SessionMerged { /* unchanged */ into_session: String, sources: Vec<String> },
    SessionSplit { /* unchanged */ from_session: String, new_sessions: Vec<String> },
    OrderPlaced {
        session_id: String,
        order_id: String,
        items: Vec<OrderItemSpec>,
    },
    OrderItemCancelled { /* unchanged */ order_id: String, item_index: usize, reason: Option<String> },
    OrderItemReturned { /* unchanged */ order_id: String, item_index: usize, qty: i64, reason: Option<String> },
    PaymentTaken { /* unchanged */ session_id: String, subtotal: i64, discount_pct: u32, vat_pct: u32, total: i64, method: String },
}
```

`event_type()` and `EventType::as_str()` unchanged (variant names are stable).

Update the existing `roundtrip_serialization` test to use the new payload:
```rust
let e = DomainEvent::SessionOpened {
    spot: SpotRef::Room { id: 1, name: "VIP-1".into(), hourly_rate: 100_000 },
    opened_by: 42,
    customer_label: Some("VIP1".into()),
    team: Some("A".into()),
};
```

- [ ] **Step 3: Run tests (will fail downstream)**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: many compile errors in apply.rs, session.rs, command files. Tasks 4-6 fix them.

- [ ] **Step 4: Commit (in progress, source not yet building)**

This commit alone leaves a build break. Combine with Task 4 (apply + session) and possibly Task 5 (commands). Recommend: don't commit yet; complete Tasks 3+4+5 then commit as one logical change.

Skip individual commit; proceed to Task 4.

---

## Task 4: Update `apply` + `SessionState`

**Files:**
- Modify: `apps/cashier/src-tauri/src/domain/session.rs`
- Modify: `apps/cashier/src-tauri/src/domain/apply.rs`

- [ ] **Step 1: SessionState.spot**

In `session.rs`:
```rust
use crate::domain::spot::SpotRef;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub status: SessionStatus,
    pub spot: SpotRef,                      // NEW (replaces room_id + table_id)
    pub opened_by: i64,
    pub customer_label: Option<String>,
    pub team: Option<String>,
    #[serde(default)]
    pub order_ids: Vec<String>,
}
```

Update the `fold` impl in `session.rs` accordingly (replace `room_id`/`table_id` reads with `spot`).

- [ ] **Step 2: Update apply branches**

In `apply.rs`:

```rust
DomainEvent::SessionOpened { spot, opened_by, customer_label, team } => {
    store.sessions.insert(ctx.aggregate_id.to_string(), SessionState {
        session_id: ctx.aggregate_id.to_string(),
        status: SessionStatus::Open,
        spot: spot.clone(),
        opened_by: *opened_by,
        customer_label: customer_label.clone(),
        team: team.clone(),
        order_ids: Vec::new(),
    });
}

DomainEvent::SessionTransferred { from: _, to } => {
    if let Some(mut s) = store.sessions.get_mut(ctx.aggregate_id) {
        s.spot = to.clone();
    }
}
```

(Other branches unchanged — only Opened and Transferred touch the new spot field.)

- [ ] **Step 3: Update apply unit tests**

The 9 tests in `apply.rs` need `SessionOpened` literals updated:
```rust
fn opened(opener: i64) -> DomainEvent {
    DomainEvent::SessionOpened {
        spot: SpotRef::Room { id: 1, name: "R1".into(), hourly_rate: 50_000 },
        opened_by: opener,
        customer_label: Some("VIP".into()), team: None,
    }
}
```

The `transfer_updates_room_and_table` test renames to `transfer_updates_spot`:
```rust
#[test]
fn transfer_updates_spot() {
    let s = AggregateStore::new();
    apply(&s, &opened(1), ApplyCtx { aggregate_id: "a" }).unwrap();
    apply(&s, &DomainEvent::SessionTransferred {
        from: SpotRef::Room { id: 1, name: "R1".into(), hourly_rate: 50_000 },
        to: SpotRef::Table { id: 7, name: "T7".into(), room_id: None, room_name: None },
    }, ApplyCtx { aggregate_id: "a" }).unwrap();
    let r = s.sessions.get("a").unwrap();
    assert!(r.spot.is_table());
    assert_eq!(r.spot.id(), 7);
}
```

For `OrderPlaced` tests, update `OrderItemSpec` literals to include the new fields:
```rust
OrderItemSpec {
    product_id: 1, product_name: "Bia 333".into(),
    qty: 1, unit_price: 100, note: None,
    route: Route::Bar,
    recipe_snapshot: vec![],
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib domain::apply`
Expected: 9 tests pass.

(Other tests still fail compilation — Task 5 fixes integration tests.)

---

## Task 5: Refactor commands + projection_integration test

**Files:**
- Modify: `apps/cashier/src-tauri/src/commands/catalog_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/commands/session_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/commands/order_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/services/event_service.rs`
- Modify: `apps/cashier/src-tauri/src/services/command_service.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: All tests under `apps/cashier/src-tauri/tests/`

- [ ] **Step 1: `catalog_cmd::list_spots` replaces `list_rooms`/`list_tables`**

```rust
use crate::store::master::Spot;

#[tauri::command]
pub fn list_spots(state: State<'_, AppState>, token: String) -> Result<Vec<Spot>, AppError> {
    state.auth.verify(&token)?;
    state.master.lock().unwrap().list_spots()
}
```

Remove `list_rooms` and `list_tables` from `catalog_cmd.rs`. Update `lib.rs::invoke_handler` to register `list_spots` instead.

- [ ] **Step 2: `event_service::WriteCtx` gains `actor_name`**

```rust
pub struct WriteCtx<'a> {
    pub aggregate_id: &'a str,
    pub actor_staff: Option<i64>,
    pub actor_name: Option<&'a str>,        // NEW — plaintext, written to event row column
    pub at: Option<DateTime<Utc>>,
}
```

In `event_service::write`, pass `actor_name` to `EventStore::append` (which gains a corresponding field).

- [ ] **Step 3: `EventStore::AppendEvent` + `EventRow` gain `actor_name`**

In `events.rs`:
```rust
pub struct AppendEvent<'a> {
    /* existing */
    pub actor_name: Option<&'a str>,
}

pub struct EventRow {
    /* existing */
    pub actor_name: Option<String>,
}
```

Update SQL INSERT and SELECTs to include the new column. Update `row_to_event` mapper.

- [ ] **Step 4: `CommandService::execute` accepts + plumbs `actor_name`**

Add `actor_name: Option<&str>` parameter to `execute`. Pass into `WriteCtx`. The actor's name is looked up from `Staff` at the call-site (commands fetch staff by `actor.staff_id` then pass `Some(&staff.name)`).

Or simpler: have `CommandService::execute` itself fetch the staff name from master before writing. Pros: callers don't repeat the lookup. Cons: extra master query per command.

**Recommend the lookup-inside-execute approach** to keep call sites tidy:
```rust
let actor_name = self.master.lock().unwrap()
    .get_staff(effective_actor_staff)?.map(|s| s.name);
// pass actor_name.as_deref() into WriteCtx
```

This adds one master query per command. Acceptable cost; cached via the in-memory store later if needed.

- [ ] **Step 5: Update `session_cmd::open_session`**

Input takes `spot_id: i64` (not separate room_id/table_id). Command:
```rust
let actor = state.auth.verify(&input.token)?;
let spot = state.master.lock().unwrap().get_spot(input.spot_id)?
    .ok_or(AppError::NotFound)?;
let spot_ref = match spot.kind {
    SpotKind::Room => SpotRef::Room {
        id: spot.id, name: spot.name,
        hourly_rate: spot.hourly_rate.ok_or_else(|| AppError::Validation("room missing rate".into()))?,
    },
    SpotKind::Table => {
        let (room_id, room_name) = if let Some(pid) = spot.parent_id {
            let parent = state.master.lock().unwrap().get_spot(pid)?;
            match parent {
                Some(p) => (Some(p.id), Some(p.name)),
                None => (None, None),
            }
        } else { (None, None) };
        SpotRef::Table { id: spot.id, name: spot.name, room_id, room_name }
    }
};
let event = DomainEvent::SessionOpened {
    spot: spot_ref,
    opened_by: actor.staff_id,
    customer_label: input.customer_label,
    team: input.team,
};
// then state.commands.execute(...)
```

Drop `room_id`/`table_id` from `OpenSessionInput`; replace with `spot_id`.

Update `transfer_session` similarly: input has `to_spot_id`, resolve to SpotRef, derive `from` from current SessionState.spot.

- [ ] **Step 6: Update `order_cmd::place_order`**

Input takes `items: [{product_id, qty, note?}]`. Command resolves each:
```rust
let master = state.master.lock().unwrap();
let mut spec_items = Vec::with_capacity(input.items.len());
for raw in input.items {
    let p = master.get_product(raw.product_id)?
        .ok_or_else(|| AppError::Validation(format!("product {} not found", raw.product_id)))?;
    let recipe = master.get_recipe(p.id)?;     // Vec<RecipeIngredient> — see Task 6
    let recipe_snapshot = recipe.into_iter().map(|ing| RecipeIngredientSnapshot {
        ingredient_id: ing.ingredient_id,
        ingredient_name: ing.ingredient_name,
        qty: ing.qty,
        unit: ing.unit,
    }).collect();
    spec_items.push(OrderItemSpec {
        product_id: p.id, product_name: p.name,
        qty: raw.qty, unit_price: p.price,
        note: raw.note,
        route: parse_route(&p.route)?,        // map "kitchen"/"bar"/"none" → Route enum
        recipe_snapshot,
    });
}
drop(master);
let event = DomainEvent::OrderPlaced {
    session_id: input.session_id.clone(),
    order_id: order_id.clone(),
    items: spec_items,
};
```

(`get_product` and `get_recipe` are added in Task 6.)

- [ ] **Step 7: Defer commit until Task 6 lands**

Source side won't fully build without `get_product` and `get_recipe`. Continue to Task 6.

---

## Task 6: `Master::get_product` + `get_recipe` (with ingredient name)

**Files:** Modify `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: Product getter**

```rust
impl Master {
    pub fn get_product(&self, id: i64) -> AppResult<Option<Product>> {
        Ok(self.conn.query_row(
            "SELECT id, name, price, route, kind FROM product WHERE id = ?1",
            params![id], |r| Ok(Product {
                id: r.get(0)?, name: r.get(1)?, price: r.get(2)?,
                route: r.get(3)?, kind: r.get(4)?,
            }),
        ).optional()?)
    }
}
```

(`Product` struct already exists from Plan D Task 8.)

- [ ] **Step 2: Recipe getter with ingredient name**

```rust
#[derive(Debug, Clone, Serialize)]
pub struct RecipeIngredient {
    pub ingredient_id: i64,
    pub ingredient_name: String,    // joined from product table
    pub qty: f64,
    pub unit: String,
}

impl Master {
    /// Get recipe for `product_id` — joins recipe rows against product table to
    /// resolve ingredient_name. Used at order-write time to snapshot the recipe
    /// into the event payload.
    pub fn get_recipe(&self, product_id: i64) -> AppResult<Vec<RecipeIngredient>> {
        let mut stmt = self.conn.prepare(
            "SELECT r.ingredient_id, p.name, r.qty, r.unit
             FROM recipe r
             JOIN product p ON p.id = r.ingredient_id
             WHERE r.product_id = ?1
             ORDER BY r.ingredient_id ASC")?;
        let rows = stmt.query_map(params![product_id], |r| Ok(RecipeIngredient {
            ingredient_id: r.get(0)?, ingredient_name: r.get(1)?,
            qty: r.get(2)?, unit: r.get(3)?,
        }))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
```

- [ ] **Step 3: Tests**

Add unit tests in `master.rs`:
- `get_product_returns_none_for_missing_id`
- `get_product_returns_full_row`
- `get_recipe_returns_empty_for_no_recipe`
- `get_recipe_includes_ingredient_name`

You'll need to insert product + recipe rows manually via `conn.execute` since there's no `create_recipe` helper.

- [ ] **Step 4: Run all tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: source compiles. Many integration tests still fail because `OrderItemSpec` literals + `SessionOpened` literals need updates.

- [ ] **Step 5: Big commit (Tasks 3-6 together)**

```bash
git add -u
git commit -m "feat(cashier): event payload snapshots — SpotRef + product/recipe in OrderItemSpec

Schema: spot table replaces room+table (one-of via SpotKind).
Domain: SpotRef enum (Room|Table) snapshotted into SessionOpened/Transferred;
OrderItemSpec gains product_name, route (Route enum), recipe_snapshot
(with ingredient_name); RecipeIngredientSnapshot for self-contained inventory.
Master: list_spots/get_spot/create_spot replace list_rooms/list_tables;
get_product/get_recipe added; get_recipe joins product table for ingredient_name.
Commands: open_session/transfer_session take spot_id; place_order takes
items: [{product_id, qty, note}] and resolves snapshots at write time.
EventStore: actor_name plaintext column; WriteCtx + AppendEvent + EventRow
gain actor_name; CommandService::execute looks up staff name and plumbs through.

Events are now self-contained: rename a spot or product, history reproduces."
```

---

## Task 7: Update all integration tests

**Files:** All `apps/cashier/src-tauri/tests/*.rs`

Each integration test that constructs `DomainEvent::SessionOpened`, `OrderItemSpec`, or `DomainEvent::SessionTransferred` literals needs updating. Establish a `tests/common/mod.rs` with helpers if useful, or update each file inline.

- [ ] **Step 1: Helper module** (optional, recommended)

Create `apps/cashier/src-tauri/tests/common/mod.rs`:
```rust
use cashier_lib::domain::event::{OrderItemSpec, RecipeIngredientSnapshot, Route};
use cashier_lib::domain::spot::SpotRef;

pub fn room(id: i64) -> SpotRef {
    SpotRef::Room { id, name: format!("R{id}"), hourly_rate: 50_000 }
}
pub fn table(id: i64) -> SpotRef {
    SpotRef::Table { id, name: format!("T{id}"), room_id: None, room_name: None }
}
pub fn item(product_id: i64, qty: i64, unit_price: i64) -> OrderItemSpec {
    OrderItemSpec {
        product_id, product_name: format!("P{product_id}"),
        qty, unit_price, note: None,
        route: Route::Bar,
        recipe_snapshot: vec![],
    }
}
```

Then in each test file: `mod common;` and `use common::{room, table, item};`.

- [ ] **Step 2: Sweep each test file**

Files to update (`grep -l 'room_id:\|SessionOpened\|OrderItemSpec' tests/`):
- `aggregate_store_integration.rs`
- `commands_integration.rs`
- `hardening_integration.rs`
- `override_integration.rs`
- `projection_integration.rs`
- `remaining_commands_integration.rs`
- `validation_integration.rs`

Pattern per test:
- `room_id: Some(1), table_id: None` → `spot: room(1)` (use helper)
- `OrderItemSpec { product_id: 1, qty: 2, unit_price: 50_000, note: None }` → `item(1, 2, 50_000)` or expanded form

Some tests assert on `state.room_id` or `state.table_id` — change to `state.spot.is_room()`/`is_table()` and `state.spot.id()`.

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all 149 tests pass + new master tests = ~158-160.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "test(cashier): update all tests for SpotRef + OrderItemSpec snapshots

Adds tests/common/mod.rs with room()/table()/item() helpers."
```

---

## Task 8: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — all pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean (commit `chore(cashier): cargo fmt` if needed)
- [ ] `pnpm --filter @tableorder/cashier typecheck` clean

---

## Done

End state:
- `spot` table replaces `room` + `table`. `kind: 'room'|'table'` discriminator. `hourly_rate` only when kind=room.
- `SpotRef` enum (Room | Table) in domain — type-enforces one-of.
- `SessionOpened`/`SessionTransferred` carry full `SpotRef` snapshot — historical sessions reproduce regardless of master-data edits.
- `OrderItemSpec` carries `product_name`, `route` (Route enum), and `recipe_snapshot` (with `ingredient_name`).
- `event` row has plaintext `actor_name` column for audit display without decrypt.
- `CommandService::execute` resolves all snapshots at write time via Master lookups.
- All integration tests updated; helper module simplifies fixtures.

What this unblocks:
- **Plan E1** can expose commands to HTTP/tablets without locking in payload shape changes mid-flight.
- **Plan F EOD reports** can render historical line items without joining against master data.
- **Inventory deltas** (Plan F) consume `recipe_snapshot` directly.
- **Spot rename / product rename** in master is safe — never affects history.
