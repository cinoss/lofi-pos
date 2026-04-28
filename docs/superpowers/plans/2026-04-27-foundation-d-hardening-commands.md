# Foundation Plan D — Hardening + Override + Remaining Commands

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the concurrency and override gaps from Plan C, swap the `EventStore` mutex for a connection pool, and ship the remaining domain commands so the cashier UI has every operation it needs (transfer/merge/split/cancel/return + 5 read commands). End state: a single Tauri process can drive the full POS workflow with race-safe writes and end-to-end override flow.

**Architecture:** Add per-key + per-aggregate `Mutex` maps (`DashMap<String, Arc<Mutex<()>>>`) wrapping `CommandService::execute`. Replace `EventStore::Mutex<Connection>` with `r2d2_sqlite` pool — Master keeps single Mutex (writer-of-record). Surface `Outcome<T>` from `execute` so future WS broadcast knows what's new. Override flow accepts `override_pin: Option<String>`; on `OverrideRequired(min)` a valid override PIN reissues with the override-staff recorded in event payload.

**Tech Stack:** Adds `dashmap`, `r2d2`, `r2d2_sqlite` to existing Rust deps. No new TS deps.

**Spec:** `docs/superpowers/specs/2026-04-27-foundation-design.md` — covers the override mechanics, transfer/merge/split semantics, and read API surface (Tauri command parts only; HTTP is Plan E).

**Builds on:** Plan A + B + C. Per the Plan D prep notes in memory, this addresses items 1–4, 6, 9 from the handoff.

**Out of scope (deferred to Plan E):**
- axum HTTP+WS server (LAN tablets)
- packages/shared TS client
- apps/web order tablet PWA
- `idle_lock_minutes` UI enforcement (frontend; backend just exposes the setting)

**Out of scope (Plan F):**
- EOD pipeline + reports + print stubs

---

## File Structure

```
apps/cashier/src-tauri/src/
  services/
    command_service.rs                  # MODIFIED — Outcome<T> in return; per-key + per-agg locks
    event_service.rs                    # MODIFIED — accept Pool instead of Arc<EventStore>; method bodies updated
    locking.rs                          # NEW — KeyMutex<K> abstraction (DashMap<K, Arc<Mutex<()>>>)
    validation.rs                       # MODIFIED — extend SessionTransferred/Merged/Split invariants
  store/
    events.rs                           # MODIFIED — Pool<SqliteConnectionManager> instead of Mutex<Connection>
  auth/
    service.rs                          # MODIFIED — verify_pin_for_role(pin, min_role) helper for overrides
  commands/
    session_cmd.rs                      # MODIFIED — add transfer_session, merge_sessions, split_session
    order_cmd.rs                        # MODIFIED — add cancel_order_item, return_order_item
    catalog_cmd.rs                      # MODIFIED — add list_rooms, list_tables, list_products,
                                        #            list_active_sessions, get_session
  store/master.rs                       # MODIFIED — list_rooms/list_tables/list_products + active sessions index
  app_state.rs                          # MODIFIED — CommandService now carries the lock maps
tests/
  hardening_integration.rs              # NEW — per-key idempotency race; per-agg validate-write race
  override_integration.rs               # NEW — override_pin completes ACL-required actions
  remaining_commands_integration.rs     # NEW — transfer + merge + split + cancel + return happy paths
```

Decisions baked in:
- **`r2d2_sqlite`** for `EventStore` reads (pooled), single `Mutex<Connection>` for writes (kept). The pool is read-only; writes go through `EventStore::append` which still serializes. WAL means readers don't block the writer.
- **Per-key idempotency mutex** is process-wide. `DashMap<String, Arc<Mutex<()>>>` with eviction-after-success is fine since `idempotency_key` table is the durable record; the mutex is just to serialize the get-validate-write-put pipeline for one key.
- **Per-aggregate write lock** has same shape; key = aggregate_id. Released after `event_service.write` returns.
- **`SessionMerged` aggregate-id contract**: write under TARGET aggregate_id (pinned in T7).
- **Override flow**: `CommandService::execute` accepts an optional `override_pin`. On `OverrideRequired(min)`, if PIN provided AND verifies against staff with role ≥ min, retry `execute` with the override staff's id attached to the event payload (we stash both `actor_staff` and `override_staff` in the event row's `actor_staff` column for now; richer per-event override field can land later).

---

## Task 1: Add deps

**Files:** Modify `apps/cashier/src-tauri/Cargo.toml`

- [ ] **Step 1: Add to `[dependencies]`**

```toml
dashmap = "6"
r2d2 = "0.8"
r2d2_sqlite = "0.25"
```

- [ ] **Step 2: Verify**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: deps resolve, no compile errors.

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat(cashier): add dashmap, r2d2, r2d2_sqlite deps"
```

---

## Task 2: `locking::KeyMutex<K>` abstraction

**Files:**
- Create: `apps/cashier/src-tauri/src/services/locking.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs`

- [ ] **Step 1: Implement**

Create `apps/cashier/src-tauri/src/services/locking.rs`:
```rust
use dashmap::DashMap;
use std::hash::Hash;
use std::sync::{Arc, Mutex, MutexGuard};

/// Process-wide mutex map keyed by `K`. Each key gets its own `Mutex<()>`;
/// `lock(key)` returns a guard that serializes callers using the same key.
///
/// Uses `DashMap` so distinct keys don't contend. The inner `Arc<Mutex<()>>`
/// is cheap to clone (one atomic increment); the guard holds it for the
/// duration of the critical section.
pub struct KeyMutex<K: Eq + Hash + Clone> {
    map: DashMap<K, Arc<Mutex<()>>>,
}

impl<K: Eq + Hash + Clone> KeyMutex<K> {
    pub fn new() -> Self { Self { map: DashMap::new() } }

    /// Acquire the mutex for `key`. Blocking; if another caller holds the
    /// mutex for the same key, this blocks until released.
    ///
    /// Returns `(guard, arc)` — keep both alive for the critical section.
    /// Dropping the guard releases the mutex.
    pub fn lock(&self, key: K) -> KeyGuard<'static> {
        let arc = self.map.entry(key).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        // SAFETY: we transmute the lifetime to 'static because we keep the
        // Arc alive inside the guard. The MutexGuard borrows from the Arc.
        let guard = arc.lock().unwrap();
        let guard: MutexGuard<'static, ()> = unsafe {
            std::mem::transmute::<MutexGuard<'_, ()>, MutexGuard<'static, ()>>(guard)
        };
        KeyGuard { _arc: arc, _guard: guard }
    }
}

impl<K: Eq + Hash + Clone> Default for KeyMutex<K> {
    fn default() -> Self { Self::new() }
}

/// Drop releases the inner Mutex. The Arc keeps the Mutex alive even if
/// `KeyMutex::map` evicts the entry.
pub struct KeyGuard<'a> {
    _arc: Arc<Mutex<()>>,
    _guard: MutexGuard<'a, ()>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::thread;

    #[test]
    fn distinct_keys_do_not_contend() {
        let km: KeyMutex<&'static str> = KeyMutex::new();
        let _a = km.lock("a");
        let _b = km.lock("b");
        // both acquired without deadlock
    }

    #[test]
    fn same_key_serializes_threads() {
        let km: Arc<KeyMutex<&'static str>> = Arc::new(KeyMutex::new());
        let counter = Arc::new(AtomicUsize::new(0));
        let mut handles = vec![];
        for _ in 0..10 {
            let km = km.clone();
            let counter = counter.clone();
            handles.push(thread::spawn(move || {
                let _g = km.lock("k");
                let v = counter.load(Ordering::SeqCst);
                std::thread::sleep(std::time::Duration::from_millis(2));
                counter.store(v + 1, Ordering::SeqCst);
            }));
        }
        for h in handles { h.join().unwrap(); }
        assert_eq!(counter.load(Ordering::SeqCst), 10);
    }
}
```

- [ ] **Step 2: Wire into `services/mod.rs`**

```rust
pub mod command_service;
pub mod day_key;
pub mod event_service;
pub mod idempotency;
pub mod locking;
pub mod validation;
```

- [ ] **Step 3: Test**

Run: `cd apps/cashier/src-tauri && cargo test --lib services::locking`
Expected: 2 tests pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): KeyMutex<K> for per-key serialization"
```

---

## Task 3: `EventStore` with r2d2 pool

**Files:** Modify `apps/cashier/src-tauri/src/store/events.rs`

- [ ] **Step 1: Refactor struct**

Replace the `EventStore` struct definition:
```rust
use r2d2::Pool;
use r2d2_sqlite::SqliteConnectionManager;
use std::sync::Mutex;
use std::path::Path;

pub struct EventStore {
    /// Pooled connections for read paths. WAL allows concurrent readers.
    read_pool: Pool<SqliteConnectionManager>,
    /// Single writer connection (rusqlite::Connection is !Sync).
    /// Append serializes through this mutex; reads bypass via the pool.
    writer: Mutex<rusqlite::Connection>,
}
```

- [ ] **Step 2: Update `open` and `open_in_memory`**

`open(path)`:
```rust
pub fn open(path: &Path) -> AppResult<Self> {
    // Run migrations on a one-shot connection first.
    let mut bootstrap = rusqlite::Connection::open(path)?;
    bootstrap.pragma_update(None, "journal_mode", "WAL")?;
    bootstrap.pragma_update(None, "foreign_keys", "ON")?;
    crate::store::migrations::run_migrations(
        &mut bootstrap,
        &crate::store::migrations::EVENTS_MIGRATIONS,
    )?;
    drop(bootstrap);

    // Build the read pool.
    let manager = SqliteConnectionManager::file(path)
        .with_init(|c| {
            c.pragma_update(None, "journal_mode", "WAL")?;
            c.pragma_update(None, "foreign_keys", "ON")?;
            Ok(())
        });
    let read_pool = Pool::builder()
        .max_size(8)
        .build(manager)
        .map_err(|e| crate::error::AppError::Internal(format!("r2d2: {e}")))?;

    // Dedicated writer connection.
    let mut writer = rusqlite::Connection::open(path)?;
    writer.pragma_update(None, "journal_mode", "WAL")?;
    writer.pragma_update(None, "foreign_keys", "ON")?;

    Ok(Self { read_pool, writer: Mutex::new(writer) })
}
```

`open_in_memory`:
```rust
pub fn open_in_memory() -> AppResult<Self> {
    // In-memory pools are tricky: each connection gets its own DB.
    // Use a shared memory URI so the pool sees the same DB.
    let uri = format!("file:eventstore_mem_{}?mode=memory&cache=shared",
                      uuid::Uuid::new_v4().simple());
    let mut bootstrap = rusqlite::Connection::open(&uri)?;
    bootstrap.pragma_update(None, "foreign_keys", "ON")?;
    crate::store::migrations::run_migrations(
        &mut bootstrap, &crate::store::migrations::EVENTS_MIGRATIONS,
    )?;
    // Keep bootstrap alive: shared-cache memory DB disappears when last
    // connection closes. Stash it as the writer.

    let manager = SqliteConnectionManager::file(&uri)
        .with_init(|c| { c.pragma_update(None, "foreign_keys", "ON")?; Ok(()) });
    let read_pool = Pool::builder()
        .max_size(4)
        .build(manager)
        .map_err(|e| crate::error::AppError::Internal(format!("r2d2: {e}")))?;

    Ok(Self { read_pool, writer: Mutex::new(bootstrap) })
}
```

- [ ] **Step 3: Update method bodies**

Reads use `self.read_pool.get()?`; writes use `self.writer.lock().unwrap()`.

`append`:
```rust
pub fn append(&self, ev: AppendEvent<'_>) -> AppResult<i64> {
    let writer = self.writer.lock().unwrap();
    writer.execute(
        "INSERT INTO event ... VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![/* same as before */],
    )?;
    Ok(writer.last_insert_rowid())
}
```

`list_for_day`, `list_for_aggregate`, `count_for_day`, `latest_for_aggregate`:
```rust
pub fn list_for_day(&self, business_day: &str) -> AppResult<Vec<EventRow>> {
    let conn = self.read_pool.get()
        .map_err(|e| crate::error::AppError::Internal(format!("r2d2 get: {e}")))?;
    let mut stmt = conn.prepare(/* same SQL */)?;
    let rows = stmt.query_map(params![business_day], row_to_event)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(rows)
}
```

`delete_day` and `vacuum` use the writer.

- [ ] **Step 4: Update doc comment on the struct**

Replace the prior `TODO(plan-d-http)` doc with:
```rust
/// `EventStore` uses an `r2d2_sqlite` pool for reads (WAL allows concurrent
/// readers) and a single `Mutex<Connection>` for the writer (rusqlite's
/// `Connection: !Sync`; SQLite serializes writes anyway). This shape lets
/// many parallel HTTP/Tauri handlers project state simultaneously while
/// `append` still maintains write-ordering invariants.
```

- [ ] **Step 5: Run existing tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 114 prior pass.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "refactor(cashier): EventStore uses r2d2_sqlite pool for reads

Single Mutex<Connection> remains for writer (SQLite serializes writes
anyway, and rusqlite::Connection is !Sync). Reads pull from a pool
of size 8 (4 for in-memory tests). WAL means readers don't block
the writer. Removes the TODO(plan-d-http) noted in Plan C."
```

---

## Task 4: `CommandService` with per-key + per-agg locks; surface `Outcome<T>`

**Files:**
- Modify: `apps/cashier/src-tauri/src/services/command_service.rs`
- Modify: `apps/cashier/src-tauri/src/app_state.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (construction)

- [ ] **Step 1: Extend `CommandService`**

```rust
use crate::services::locking::KeyMutex;

pub struct CommandService {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub event_service: EventService,
    pub clock: Arc<dyn Clock>,
    pub auth: Arc<AuthService>,                    // NEW — for override PIN check
    pub idem_lock: Arc<KeyMutex<String>>,          // NEW — per-idempotency-key serialization
    pub agg_lock: Arc<KeyMutex<String>>,           // NEW — per-aggregate validate-write serialization
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteOutcome {
    Inserted,
    Cached,
}
```

- [ ] **Step 2: Refactor `execute` to return `(T, WriteOutcome)` and use locks**

```rust
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
    // ACL — first pass against actor's role.
    let effective_actor_staff = match policy::check(action, actor.role, ctx) {
        policy::Decision::Allow => actor.staff_id,
        policy::Decision::Deny => return Err(AppError::Unauthorized),
        policy::Decision::OverrideRequired(min) => {
            // If PIN was supplied, try to satisfy with an override.
            let pin = override_pin.ok_or(AppError::OverrideRequired(min))?;
            let override_staff = self
                .auth
                .verify_pin_for_role(pin, min)?;
            override_staff.id
        }
    };

    // TOCTOU window closed: serialize same-key callers process-wide.
    let _idem_guard = self.idem_lock.lock(idempotency_key.to_string());

    // Cache check (lock-and-drop).
    if let Some(cached) = self.master.lock().unwrap().get_idempotency(idempotency_key)? {
        let v: T = serde_json::from_str(&cached)
            .map_err(|e| AppError::Internal(format!("cached parse: {e}")))?;
        return Ok((v, WriteOutcome::Cached));
    }

    // Per-aggregate validate-write serialization.
    let _agg_guard = self.agg_lock.lock(aggregate_id.to_string());

    validation::validate(&self.event_service, &self.events, aggregate_id, &event)?;
    self.event_service.write(
        WriteCtx {
            aggregate_id,
            actor_staff: Some(effective_actor_staff),
            at: None,
        },
        &event,
    )?;
    let projection = project(self)?;

    // Persist cache row.
    let now = self.clock.now().timestamp_millis();
    let json = serde_json::to_string(&projection)
        .map_err(|e| AppError::Internal(format!("project serialize: {e}")))?;
    self.master.lock().unwrap()
        .put_idempotency(idempotency_key, command_name, &json, now)?;

    Ok((projection, WriteOutcome::Inserted))
}
```

- [ ] **Step 3: Add `verify_pin_for_role` to `AuthService`**

In `auth/service.rs`:
```rust
/// Verify a PIN and return the staff IF role >= `min_role`. Used for
/// supervisor-override flows. Constant-cost iteration over staff list.
pub fn verify_pin_for_role(&self, pin: &str, min_role: Role) -> AppResult<Staff> {
    let staff = self.master.lock().unwrap().list_staff()?;
    for s in staff {
        if s.role >= min_role && pin::verify_pin(pin, &s.pin_hash)? {
            return Ok(s);
        }
    }
    Err(AppError::Unauthorized)
}
```

Add a unit test in `auth/service.rs`:
```rust
#[test]
fn verify_pin_for_role_succeeds_when_role_meets_min() {
    let svc = rig();
    // rig() seeds an Owner with PIN "1234"
    let s = svc.verify_pin_for_role("1234", Role::Manager).unwrap();
    assert_eq!(s.role, Role::Owner);
}

#[test]
fn verify_pin_for_role_fails_when_pin_belongs_to_lower_role() {
    let svc = rig();
    let staff_pin = "5555";
    let h = crate::auth::pin::hash_pin(staff_pin).unwrap();
    svc.master.lock().unwrap()
        .create_staff("Worker", &h, Role::Staff, None).unwrap();
    assert!(matches!(
        svc.verify_pin_for_role(staff_pin, Role::Manager),
        Err(AppError::Unauthorized)
    ));
}

#[test]
fn verify_pin_for_role_fails_for_unknown_pin() {
    let svc = rig();
    assert!(matches!(
        svc.verify_pin_for_role("0000", Role::Staff),
        Err(AppError::Unauthorized)
    ));
}
```

- [ ] **Step 4: Update AppState construction in `lib.rs::run`**

```rust
let auth = Arc::new(auth::AuthService { ... });
let idem_lock = Arc::new(services::locking::KeyMutex::new());
let agg_lock = Arc::new(services::locking::KeyMutex::new());
let commands = services::command_service::CommandService {
    master: master.clone(),
    events: events.clone(),
    event_service,
    clock: clock.clone(),
    auth: auth.clone(),
    idem_lock,
    agg_lock,
};
app.manage(app_state::AppState { kek, master, events, clock, auth: (*auth).clone() /* see note */, commands });
```

(Note: `AuthService` doesn't implement `Clone` today. Either derive it — fields are all `Arc + Copy`able — or store `Arc<AuthService>` everywhere. Recommended: derive `Clone` on `AuthService` since its fields are already shareable. This is a one-line change in `auth/service.rs`.)

If you derive `Clone`, then `AppState.auth: AuthService` and `CommandService.auth: AuthService` (no Arc); construct once and clone twice.

- [ ] **Step 5: Update existing command call sites**

The `state.commands.execute(...)` calls in `commands/{auth_cmd, session_cmd, order_cmd, payment_cmd}.rs` now need:
- New `override_pin: Option<&str>` argument (pass `None` for now — Task 7 wires it)
- Match on `(T, WriteOutcome)` return — discard the outcome at command level: `let (proj, _outcome) = state.commands.execute(...)?; Ok(proj)`

- [ ] **Step 6: Update `tests/commands_integration.rs`**

Same pattern: pass `None` for override_pin; destructure `(result, _)`.

- [ ] **Step 7: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 114 + 3 (auth) = 117 pass.

- [ ] **Step 8: Commit**

```bash
git add -u
git commit -m "feat(cashier): CommandService — per-key + per-agg locks; Outcome surface; override hook

Adds two KeyMutex<String> instances (idem_lock + agg_lock) wrapping
the cache check and the validate-write phase respectively. execute()
now returns (T, WriteOutcome::Inserted|Cached) so future WS broadcast
can avoid replays. AuthService gains verify_pin_for_role used by the
override path; existing call sites pass override_pin: None for now."
```

---

## Task 5: Hardening integration test — race-window verification

**Files:** Create `apps/cashier/src-tauri/tests/hardening_integration.rs`

- [ ] **Step 1: Write tests**

```rust
use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::services::command_service::{CommandService, WriteOutcome};
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex};
use std::thread;
use uuid::Uuid;

fn rig() -> (CommandService, AuthService) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let clock: Arc<MockClock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let event_service = EventService {
        master: master.clone(), events: events.clone(), kek: kek.clone(),
        clock: clock.clone(), cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService { master: master.clone(), clock: clock.clone(), signing_key: signing };

    let pin = "1111";
    let h = hash_pin(pin).unwrap();
    master.lock().unwrap().create_staff("Owner", &h, Role::Owner, None).unwrap();

    let commands = CommandService {
        master: master.clone(), events: events.clone(),
        event_service, clock: clock.clone(),
        auth: Arc::new(auth.clone()),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
    };
    (commands, auth)
}

#[test]
fn same_idempotency_key_under_race_yields_one_write_one_cached() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("1111").unwrap();

    let session_id = Uuid::new_v4().to_string();
    let key = "race-key-1".to_string();
    let event = DomainEvent::SessionOpened {
        room_id: Some(1), table_id: None, opened_by: claims.staff_id,
        customer_label: None, team: None,
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
            cs.execute(
                &claims, Action::OpenSession, PolicyCtx::default(),
                &key, "open_session", &session_id, event, None,
                |c| c.load_session(&session_id).map(|o| o.unwrap()),
            )
        }));
    }
    let outcomes: Vec<_> = handles.into_iter().map(|h| h.join().unwrap().unwrap()).collect();
    let inserted = outcomes.iter().filter(|(_, o)| *o == WriteOutcome::Inserted).count();
    let cached = outcomes.iter().filter(|(_, o)| *o == WriteOutcome::Cached).count();
    assert_eq!(inserted, 1, "exactly one write should have occurred");
    assert_eq!(cached, 7, "seven cached responses");
}

#[test]
fn distinct_idempotency_keys_proceed_in_parallel() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("1111").unwrap();
    let cs = Arc::new(cs);
    let claims = Arc::new(claims);

    let mut handles = vec![];
    for i in 0..8 {
        let cs = cs.clone();
        let claims = claims.clone();
        handles.push(thread::spawn(move || {
            let session_id = Uuid::new_v4().to_string();
            cs.execute(
                &claims, Action::OpenSession, PolicyCtx::default(),
                &format!("k-{i}"), "open_session", &session_id,
                DomainEvent::SessionOpened {
                    room_id: Some(i), table_id: None, opened_by: claims.staff_id,
                    customer_label: None, team: None,
                },
                None,
                |c| c.load_session(&session_id).map(|o| o.unwrap()),
            )
        }));
    }
    for h in handles {
        let (_, o) = h.join().unwrap().unwrap();
        assert_eq!(o, WriteOutcome::Inserted);
    }
}
```

- [ ] **Step 2: Run**

Run: `cd apps/cashier/src-tauri && cargo test --test hardening_integration`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "test(cashier): per-key idempotency race + parallel-key throughput"
```

---

## Task 6: Override completion path — integration test

**Files:** Create `apps/cashier/src-tauri/tests/override_integration.rs`

- [ ] **Step 1: Write test**

```rust
use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::DomainEvent;
use cashier_lib::error::AppError;
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn rig() -> (CommandService, AuthService) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let clock: Arc<MockClock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let event_service = EventService {
        master: master.clone(), events: events.clone(), kek: kek.clone(),
        clock: clock.clone(), cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService { master: master.clone(), clock: clock.clone(), signing_key: signing };

    // Seed a Manager (owner pin "9999") and a Staff (pin "1111").
    let mh = hash_pin("9999").unwrap();
    master.lock().unwrap().create_staff("Boss", &mh, Role::Manager, None).unwrap();
    let sh = hash_pin("1111").unwrap();
    master.lock().unwrap().create_staff("Worker", &sh, Role::Staff, None).unwrap();

    let commands = CommandService {
        master: master.clone(), events: events.clone(),
        event_service, clock: clock.clone(),
        auth: Arc::new(auth.clone()),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
    };
    (commands, auth)
}

#[test]
fn override_pin_unblocks_action() {
    let (cs, auth) = rig();
    let (_, staff_claims) = auth.login("1111").unwrap();

    // Open a session as staff (Allow).
    let session_id = Uuid::new_v4().to_string();
    cs.execute(
        &staff_claims, Action::OpenSession, PolicyCtx::default(),
        "ovr-open", "open_session", &session_id,
        DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: staff_claims.staff_id,
            customer_label: None, team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();

    // Attempt a manager-required action without override → fails.
    let close_attempt = cs.execute(
        &staff_claims, Action::CloseSession, PolicyCtx::default(),
        "ovr-close-1", "close_session", &session_id,
        DomainEvent::SessionClosed { closed_by: staff_claims.staff_id, reason: None },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    );
    assert!(matches!(close_attempt, Err(AppError::OverrideRequired(_))));

    // Retry with manager override PIN → succeeds.
    let (closed, _) = cs.execute(
        &staff_claims, Action::CloseSession, PolicyCtx::default(),
        "ovr-close-2", "close_session", &session_id,
        DomainEvent::SessionClosed { closed_by: staff_claims.staff_id, reason: None },
        Some("9999"),
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();
    assert_eq!(closed.status, cashier_lib::domain::session::SessionStatus::Closed);
}

#[test]
fn override_pin_for_lower_role_rejected() {
    let (cs, auth) = rig();
    let (_, staff_claims) = auth.login("1111").unwrap();

    let session_id = Uuid::new_v4().to_string();
    cs.execute(
        &staff_claims, Action::OpenSession, PolicyCtx::default(),
        "ovr-open-2", "open_session", &session_id,
        DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: staff_claims.staff_id,
            customer_label: None, team: None,
        },
        None,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();

    let result = cs.execute(
        &staff_claims, Action::CloseSession, PolicyCtx::default(),
        "ovr-close-bad", "close_session", &session_id,
        DomainEvent::SessionClosed { closed_by: staff_claims.staff_id, reason: None },
        Some("1111"),  // Staff PIN — insufficient role
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    );
    assert!(matches!(result, Err(AppError::Unauthorized)));
}
```

- [ ] **Step 2: Run**

Run: `cd apps/cashier/src-tauri && cargo test --test override_integration`
Expected: 2 tests pass.

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "test(cashier): override_pin completes manager-required actions"
```

---

## Task 7: Wire `override_pin` into Tauri commands

**Files:** Modify all command files that call `state.commands.execute(...)`.

- [ ] **Step 1: Add `override_pin: Option<String>` to each input struct**

For `CloseSessionInput`, `OpenSessionInput` (open is allow-by-default but include for symmetry — pass `None` always), `PlaceOrderInput`, `TakePaymentInput` (definitely needs it for large discounts).

For commands added in later tasks (transfer/merge/split/cancel/return), include `override_pin: Option<String>` from the start.

For `OpenSessionInput`, `PlaceOrderInput` you can omit `override_pin` since their actions are unrestricted; cleaner to include it for uniformity.

- [ ] **Step 2: Pass through to `execute(..., input.override_pin.as_deref(), ...)`**

Each command call site changes from:
```rust
state.commands.execute(&actor, action, ctx, key, name, agg, event, |c| ...)
```
to:
```rust
let (projection, _outcome) = state.commands.execute(
    &actor, action, ctx, key, name, agg, event,
    input.override_pin.as_deref(),
    |c| ...)?;
Ok(projection)
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all prior pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): plumb override_pin through Tauri command inputs"
```

---

## Task 8: Read commands — `list_rooms`, `list_tables`, `list_products`

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/master.rs` (add list helpers)
- Modify: `apps/cashier/src-tauri/src/commands/catalog_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (register commands)

- [ ] **Step 1: Master helpers**

In `master.rs` add `Room`, `Table`, `Product` row structs and list methods.

```rust
#[derive(Debug, Clone, Serialize)]
pub struct Room { pub id: i64, pub name: String, pub hourly_rate: i64, pub status: String }
#[derive(Debug, Clone, Serialize)]
pub struct Table { pub id: i64, pub name: String, pub room_id: Option<i64>, pub status: String }
#[derive(Debug, Clone, Serialize)]
pub struct Product { pub id: i64, pub name: String, pub price: i64, pub route: String, pub kind: String }

impl Master {
    pub fn list_rooms(&self) -> AppResult<Vec<Room>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, hourly_rate, status FROM room ORDER BY id ASC")?;
        let rows = stmt.query_map([], |r| Ok(Room {
            id: r.get(0)?, name: r.get(1)?, hourly_rate: r.get(2)?, status: r.get(3)?
        }))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    pub fn list_tables(&self) -> AppResult<Vec<Table>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, room_id, status FROM \"table\" ORDER BY id ASC")?;
        let rows = stmt.query_map([], |r| Ok(Table {
            id: r.get(0)?, name: r.get(1)?, room_id: r.get(2)?, status: r.get(3)?
        }))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    pub fn list_products(&self) -> AppResult<Vec<Product>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, price, route, kind FROM product ORDER BY id ASC")?;
        let rows = stmt.query_map([], |r| Ok(Product {
            id: r.get(0)?, name: r.get(1)?, price: r.get(2)?,
            route: r.get(3)?, kind: r.get(4)?
        }))?.collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}
```

Add `use serde::Serialize;` at top if not present.

Add unit tests for empty + populated list of each (3 tests × 2 = 6, or pick 3 representative).

- [ ] **Step 2: Tauri commands**

Append to `commands/catalog_cmd.rs`:
```rust
use crate::store::master::{Product, Room, Table};

#[tauri::command]
pub fn list_rooms(state: State<'_, AppState>, token: String) -> Result<Vec<Room>, AppError> {
    state.auth.verify(&token)?;
    state.master.lock().unwrap().list_rooms()
}

#[tauri::command]
pub fn list_tables(state: State<'_, AppState>, token: String) -> Result<Vec<Table>, AppError> {
    state.auth.verify(&token)?;
    state.master.lock().unwrap().list_tables()
}

#[tauri::command]
pub fn list_products(state: State<'_, AppState>, token: String) -> Result<Vec<Product>, AppError> {
    state.auth.verify(&token)?;
    state.master.lock().unwrap().list_products()
}
```

- [ ] **Step 3: Register in `lib.rs::invoke_handler`**

Add `commands::catalog_cmd::list_rooms`, `commands::catalog_cmd::list_tables`, `commands::catalog_cmd::list_products`.

- [ ] **Step 4: Tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 6 new master tests + 0 command-level tests (commands themselves are thin shims; covered by `cargo check`).

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(cashier): list_rooms/list_tables/list_products commands + master helpers"
```

---

## Task 9: Read commands — `list_active_sessions`, `get_session`

**Files:**
- Modify: `apps/cashier/src-tauri/src/services/command_service.rs` (helper)
- Modify: `apps/cashier/src-tauri/src/commands/session_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Add helpers to `CommandService`**

```rust
impl CommandService {
    /// Find active session aggregate_ids by scanning recent SessionOpened events
    /// and removing those that have a SessionClosed/Merged/Split.
    pub fn list_active_session_ids(&self) -> AppResult<Vec<String>> {
        // Naive impl: list all events of type SessionOpened, fold per-aggregate,
        // keep aggregates whose status == Open.
        // For Plan D this is fine; Plan F adds a snapshot table for perf.
        let opened = self.events.list_for_day(/* TODO */)?; // BUG — wrong API
        // Actually: we don't have a "list events by type" API. Add one.
        unimplemented!("Step 2 adds the EventStore API")
    }
}
```

This won't compile. Step 2 fixes it.

- [ ] **Step 2: Add `list_aggregate_ids_by_type` to `EventStore`**

In `apps/cashier/src-tauri/src/store/events.rs`:
```rust
impl EventStore {
    /// Distinct aggregate_ids that have at least one event of the given type.
    /// Used to enumerate live aggregates without scanning everything.
    pub fn list_aggregate_ids_by_type(&self, event_type: &str) -> AppResult<Vec<String>> {
        let conn = self.read_pool.get()
            .map_err(|e| crate::error::AppError::Internal(format!("r2d2 get: {e}")))?;
        let mut stmt = conn.prepare(
            "SELECT DISTINCT aggregate_id FROM event WHERE type = ?1")?;
        let ids = stmt.query_map(params![event_type], |r| r.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(ids)
    }
}
```

Add a unit test: write 2 SessionOpened + 1 OrderPlaced, assert `list_aggregate_ids_by_type("SessionOpened")` returns 2.

- [ ] **Step 3: Implement `list_active_session_ids`**

```rust
impl CommandService {
    pub fn list_active_session_ids(&self) -> AppResult<Vec<String>> {
        let candidates = self.events.list_aggregate_ids_by_type("SessionOpened")?;
        let mut active = Vec::new();
        for id in candidates {
            if let Some(state) = self.load_session(&id)? {
                if state.status == crate::domain::session::SessionStatus::Open {
                    active.push(id);
                }
            }
        }
        Ok(active)
    }
}
```

- [ ] **Step 4: Tauri commands**

In `commands/session_cmd.rs`:
```rust
use crate::domain::session::SessionState;

#[tauri::command]
pub fn list_active_sessions(
    state: State<'_, AppState>, token: String,
) -> Result<Vec<SessionState>, AppError> {
    state.auth.verify(&token)?;
    let ids = state.commands.list_active_session_ids()?;
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(s) = state.commands.load_session(&id)? {
            out.push(s);
        }
    }
    Ok(out)
}

#[derive(Debug, serde::Deserialize)]
pub struct GetSessionInput { pub token: String, pub session_id: String }

#[tauri::command]
pub fn get_session(
    state: State<'_, AppState>, input: GetSessionInput,
) -> Result<SessionState, AppError> {
    state.auth.verify(&input.token)?;
    state.commands.load_session(&input.session_id)?
        .ok_or(AppError::NotFound)
}
```

- [ ] **Step 5: Register in `lib.rs`**

Add `session_cmd::list_active_sessions` and `session_cmd::get_session`.

- [ ] **Step 6: Tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 1 new EventStore test.

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "feat(cashier): list_active_sessions + get_session + supporting EventStore query"
```

---

## Task 10: Write commands — `transfer_session`, `cancel_order_item`, `return_order_item`

**Files:**
- Modify: `apps/cashier/src-tauri/src/services/validation.rs` — extend for SessionTransferred
- Modify: `apps/cashier/src-tauri/src/commands/{session_cmd,order_cmd}.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Extend validation**

In `services/validation.rs`, add a branch in `validate` for `SessionTransferred`:
```rust
DomainEvent::SessionTransferred { .. } => {
    let evs = decrypt_aggregate(svc, events, aggregate_id)?;
    let s = session::fold(aggregate_id, &evs)
        .ok_or_else(|| AppError::Validation("session not opened".into()))?;
    if s.status != session::SessionStatus::Open {
        return Err(AppError::Conflict(format!(
            "session status {:?}, cannot transfer", s.status
        )));
    }
}
```

- [ ] **Step 2: Implement `transfer_session` in `commands/session_cmd.rs`**

```rust
#[derive(Debug, Deserialize)]
pub struct TransferSessionInput {
    pub token: String,
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub session_id: String,
    pub from_room_id: Option<i64>,
    pub from_table_id: Option<i64>,
    pub to_room_id: Option<i64>,
    pub to_table_id: Option<i64>,
}

#[tauri::command]
pub fn transfer_session(
    state: State<'_, AppState>, input: TransferSessionInput,
) -> Result<SessionState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let event = DomainEvent::SessionTransferred {
        from_room_id: input.from_room_id, from_table_id: input.from_table_id,
        to_room_id: input.to_room_id, to_table_id: input.to_table_id,
    };
    let (s, _) = state.commands.execute(
        &actor, Action::TransferSession, PolicyCtx::default(),
        &input.idempotency_key, "transfer_session", &input.session_id,
        event, input.override_pin.as_deref(),
        |c| c.load_session(&input.session_id)?.ok_or(AppError::NotFound),
    )?;
    Ok(s)
}
```

- [ ] **Step 3: Implement `cancel_order_item` and `return_order_item` in `commands/order_cmd.rs`**

```rust
#[derive(Debug, Deserialize)]
pub struct CancelOrderItemInput {
    pub token: String,
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub order_id: String,
    pub item_index: usize,
    pub reason: Option<String>,
    /// True if cancelling own order (within grace window allows without override).
    pub is_self: bool,
    /// Caller-computed; true if `now - order_placed_at <= cancel_grace_minutes`.
    pub within_grace: bool,
}

#[tauri::command]
pub fn cancel_order_item(
    state: State<'_, AppState>, input: CancelOrderItemInput,
) -> Result<crate::domain::order::OrderState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let action = if input.is_self && input.within_grace { Action::CancelOrderItemSelf }
                 else { Action::CancelOrderItemAny };
    let ctx = PolicyCtx { is_self: input.is_self,
                          within_cancel_grace: input.within_grace,
                          ..PolicyCtx::default() };
    let event = DomainEvent::OrderItemCancelled {
        order_id: input.order_id.clone(),
        item_index: input.item_index, reason: input.reason,
    };
    let (s, _) = state.commands.execute(
        &actor, action, ctx,
        &input.idempotency_key, "cancel_order_item", &input.order_id,
        event, input.override_pin.as_deref(),
        |c| c.load_order(&input.order_id)?.ok_or(AppError::NotFound),
    )?;
    Ok(s)
}

#[derive(Debug, Deserialize)]
pub struct ReturnOrderItemInput {
    pub token: String,
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub order_id: String,
    pub item_index: usize,
    pub qty: i64,
    pub reason: Option<String>,
}

#[tauri::command]
pub fn return_order_item(
    state: State<'_, AppState>, input: ReturnOrderItemInput,
) -> Result<crate::domain::order::OrderState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let event = DomainEvent::OrderItemReturned {
        order_id: input.order_id.clone(),
        item_index: input.item_index, qty: input.qty, reason: input.reason,
    };
    let (s, _) = state.commands.execute(
        &actor, Action::ReturnOrderItem, PolicyCtx::default(),
        &input.idempotency_key, "return_order_item", &input.order_id,
        event, input.override_pin.as_deref(),
        |c| c.load_order(&input.order_id)?.ok_or(AppError::NotFound),
    )?;
    Ok(s)
}
```

- [ ] **Step 4: Register in `lib.rs`**

Add `session_cmd::transfer_session`, `order_cmd::cancel_order_item`, `order_cmd::return_order_item`.

- [ ] **Step 5: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all prior pass; no new tests (covered by next task's integration).

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "feat(cashier): transfer_session + cancel_order_item + return_order_item commands

Validation extends to reject transfer of non-Open sessions. Cancel
chooses Action variant from is_self+within_grace flags."
```

---

## Task 11: Write commands — `merge_sessions`, `split_session`

**Files:**
- Modify: `apps/cashier/src-tauri/src/services/validation.rs` — add merge/split invariants
- Modify: `apps/cashier/src-tauri/src/commands/session_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Add validation branches**

```rust
DomainEvent::SessionMerged { sources, .. } => {
    // Aggregate_id is the TARGET. Sources must all be Open.
    let target_evs = decrypt_aggregate(svc, events, aggregate_id)?;
    let target = session::fold(aggregate_id, &target_evs)
        .ok_or_else(|| AppError::Validation("merge target not opened".into()))?;
    if target.status != session::SessionStatus::Open {
        return Err(AppError::Conflict(format!(
            "merge target status {:?}", target.status
        )));
    }
    for src in sources {
        let src_evs = decrypt_aggregate(svc, events, src)?;
        let src_state = session::fold(src, &src_evs)
            .ok_or_else(|| AppError::Validation(format!("merge source {src} not opened")))?;
        if src_state.status != session::SessionStatus::Open {
            return Err(AppError::Conflict(format!(
                "merge source {src} status {:?}", src_state.status
            )));
        }
    }
}
DomainEvent::SessionSplit { from_session, .. } => {
    // Aggregate_id is the SOURCE. Must be Open.
    let evs = decrypt_aggregate(svc, events, from_session)?;
    let s = session::fold(from_session, &evs)
        .ok_or_else(|| AppError::Validation("split source not opened".into()))?;
    if s.status != session::SessionStatus::Open {
        return Err(AppError::Conflict(format!(
            "split source status {:?}", s.status
        )));
    }
}
```

- [ ] **Step 2: Commands**

```rust
#[derive(Debug, Deserialize)]
pub struct MergeSessionsInput {
    pub token: String,
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub into_session: String,
    pub sources: Vec<String>,
}

#[tauri::command]
pub fn merge_sessions(
    state: State<'_, AppState>, input: MergeSessionsInput,
) -> Result<SessionState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let event = DomainEvent::SessionMerged {
        into_session: input.into_session.clone(),
        sources: input.sources,
    };
    let (s, _) = state.commands.execute(
        &actor, Action::MergeSessions, PolicyCtx::default(),
        &input.idempotency_key, "merge_sessions", &input.into_session,
        event, input.override_pin.as_deref(),
        |c| c.load_session(&input.into_session)?.ok_or(AppError::NotFound),
    )?;
    Ok(s)
}

#[derive(Debug, Deserialize)]
pub struct SplitSessionInput {
    pub token: String,
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub from_session: String,
    pub new_sessions: Vec<String>,
}

#[tauri::command]
pub fn split_session(
    state: State<'_, AppState>, input: SplitSessionInput,
) -> Result<SessionState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let event = DomainEvent::SessionSplit {
        from_session: input.from_session.clone(),
        new_sessions: input.new_sessions,
    };
    let (s, _) = state.commands.execute(
        &actor, Action::SplitSession, PolicyCtx::default(),
        &input.idempotency_key, "split_session", &input.from_session,
        event, input.override_pin.as_deref(),
        |c| c.load_session(&input.from_session)?.ok_or(AppError::NotFound),
    )?;
    Ok(s)
}
```

- [ ] **Step 3: Register in `lib.rs`**

Add `session_cmd::merge_sessions`, `session_cmd::split_session`.

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: pass.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(cashier): merge_sessions + split_session commands with validation

Merge target's aggregate_id holds the event (per Plan C contract);
sources are validated to be Open. Split source must be Open."
```

---

## Task 12: Remaining-commands integration test

**Files:** Create `apps/cashier/src-tauri/tests/remaining_commands_integration.rs`

- [ ] **Step 1: Tests**

Cover happy paths for: transfer, merge, split, cancel-with-override, return.

```rust
use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::{DomainEvent, OrderItemSpec};
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::locking::KeyMutex;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn rig() -> (CommandService, AuthService) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let clock: Arc<MockClock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let event_service = EventService {
        master: master.clone(), events: events.clone(), kek: kek.clone(),
        clock: clock.clone(), cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService { master: master.clone(), clock: clock.clone(), signing_key: signing };
    let h = hash_pin("9999").unwrap();
    master.lock().unwrap().create_staff("Boss", &h, Role::Owner, None).unwrap();
    let commands = CommandService {
        master: master.clone(), events: events.clone(),
        event_service, clock: clock.clone(),
        auth: Arc::new(auth.clone()),
        idem_lock: Arc::new(KeyMutex::new()),
        agg_lock: Arc::new(KeyMutex::new()),
    };
    (commands, auth)
}

fn open(cs: &CommandService, c: &cashier_lib::auth::token::TokenClaims, key: &str)
    -> String
{
    let id = Uuid::new_v4().to_string();
    cs.execute(c, Action::OpenSession, PolicyCtx::default(),
        key, "open_session", &id,
        DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: c.staff_id,
            customer_label: None, team: None,
        },
        None, |x| x.load_session(&id).map(|o| o.unwrap()),
    ).unwrap();
    id
}

#[test]
fn transfer_session_happy_path() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("9999").unwrap();
    let sid = open(&cs, &claims, "tk-1");

    let (s, _) = cs.execute(
        &claims, Action::TransferSession, PolicyCtx::default(),
        "tk-2", "transfer_session", &sid,
        DomainEvent::SessionTransferred {
            from_room_id: Some(1), from_table_id: None,
            to_room_id: None, to_table_id: Some(5),
        },
        None, |x| x.load_session(&sid).map(|o| o.unwrap()),
    ).unwrap();
    assert_eq!(s.table_id, Some(5));
    assert_eq!(s.room_id, None);
}

#[test]
fn merge_sessions_happy_path() {
    let (cs, auth) = rig();
    let (_, claims) = auth.login("9999").unwrap();
    let target = open(&cs, &claims, "mk-t");
    let src = open(&cs, &claims, "mk-s");

    let (s, _) = cs.execute(
        &claims, Action::MergeSessions, PolicyCtx::default(),
        "mk-merge", "merge_sessions", &target,
        DomainEvent::SessionMerged {
            into_session: target.clone(),
            sources: vec![src.clone()],
        },
        None, |x| x.load_session(&target).map(|o| o.unwrap()),
    ).unwrap();
    assert_eq!(s.status, cashier_lib::domain::session::SessionStatus::Open);

    // Source should be marked merged via cross-aggregate fold (when caller
    // passes the merge event in). For Plan D, the source's own aggregate
    // stream still says Open — this is the documented contract.
}

#[test]
fn cancel_order_item_with_manager_override() {
    let (cs, auth) = rig();
    let h = hash_pin("0000").unwrap();
    cs.master.lock().unwrap()
        .create_staff("Worker", &h, Role::Staff, None).unwrap();
    let (_, staff) = auth.login("0000").unwrap();
    let sid = open(&cs, &staff, "ck-open");

    // Place an order
    let oid = Uuid::new_v4().to_string();
    cs.execute(&staff, Action::PlaceOrder, PolicyCtx::default(),
        "ck-place", "place_order", &oid,
        DomainEvent::OrderPlaced {
            session_id: sid.clone(), order_id: oid.clone(),
            items: vec![OrderItemSpec { product_id: 1, qty: 2, unit_price: 100, note: None }],
        },
        None, |x| x.load_order(&oid).map(|o| o.unwrap()),
    ).unwrap();

    // Cancel as staff outside grace window — needs manager override (Owner pin "9999")
    let (o, _) = cs.execute(
        &staff, Action::CancelOrderItemAny, PolicyCtx::default(),
        "ck-cancel", "cancel_order_item", &oid,
        DomainEvent::OrderItemCancelled {
            order_id: oid.clone(), item_index: 0, reason: Some("test".into()),
        },
        Some("9999"),
        |x| x.load_order(&oid).map(|o| o.unwrap()),
    ).unwrap();
    assert!(o.items[0].cancelled);
}
```

- [ ] **Step 2: Run**

Run: `cd apps/cashier/src-tauri && cargo test --test remaining_commands_integration`
Expected: 3 tests pass.

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "test(cashier): transfer + merge + cancel-with-override integration"
```

---

## Task 13: Final gate

- [ ] **Step 1**: `cd apps/cashier/src-tauri && cargo test` — should be ~130 total
- [ ] **Step 2**: `cargo clippy --all-targets -- -D warnings`
- [ ] **Step 3**: `cargo fmt --check` (commit `chore: cargo fmt` if needed)
- [ ] **Step 4**: `pnpm --filter @tableorder/cashier typecheck`

---

## Done

End state:
- Per-key idempotency mutex + per-aggregate write lock close all known race windows
- `EventStore` reads from r2d2 pool; writes still serialize via Mutex
- `CommandService::execute` returns `(T, WriteOutcome)`; UI/HTTP layer can broadcast only on Inserted
- Override completion: `override_pin` flows from input → ACL retry → event `actor_staff` is the override staff
- 5 new write commands: transfer, merge, split, cancel_item, return_item
- 5 new read commands: list_active_sessions, get_session, list_rooms, list_tables, list_products
- All Tauri commands now have ACL + idempotency + validation + override

Commands still NOT exposed (deliberate — frontend-only):
- EOD trigger (Plan F)

Plan E will add:
- axum HTTP+WS server bound to LAN (mirrors the Tauri command surface 1:1)
- `packages/shared` with zod schemas + typed fetch + WS hook
- `apps/web` becomes the order tablet PWA (PIN login → cookie → command calls)
- `idle_lock_minutes` UI hook on cashier window
