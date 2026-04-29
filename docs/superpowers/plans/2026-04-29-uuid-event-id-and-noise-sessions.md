# UUID Event IDs + Fake Session Noise — Implementation Plan

> **SUPERSEDED.** Direction changed to a `bouncer` sidecar HTTP service that holds seeds (and printers + reports). Local UTC rotation, KEK in keychain, dek table, and noise injection all replaced by the simpler "derive DEK from sidecar seed + day" model. See `2026-04-29-bouncer-sidecar-design.md`. Do NOT execute this plan.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Two changes that ship together because one enables the other:

1. **UUID v4 for `event.id`** (was autoincrement). Sort events by `ts` (already exists) with `id` as tiebreaker. Removes the temporal information leak from row IDs in forensically-recovered free pages.

2. **Fake-session noise injection at EOD.** Just before the existing event-row delete in the EOD pipeline, synthesize N causally-valid fake sessions (SessionOpened → OrderPlaced × M → PaymentTaken → SessionClosed) with random spots/staff/products/timings spread across the business day. Each event gets its own UUID and a synthesized `ts` within day bounds. Encrypted with the same DEK as real events, written directly into events.db, then immediately bulk-deleted alongside the real events. Free pages on the SSD now contain a mix of real + fake ciphertext that decrypts to plausible session chains; attacker has no per-row distinguisher.

**Why ship together:** noise injection is only effective if the row metadata (id) doesn't leak insertion order. UUID + ts-based ordering is the prerequisite.

**Threat scope:** forensic recovery of freed pages within the 4-day DEK-retention window, with attacker who has KEK access. Out of scope (separate concerns acknowledged): warm-machine real-time observation, coercion, plaintext-report leak.

---

## File Structure

```
apps/cashier/src-tauri/src/
  store/
    events.rs                              # MODIFIED — id: Uuid; ORDER BY ts, id
    migrations/events/
      0004_event_id_uuid.sql               # NEW — recreate event with TEXT id
    aggregate_store.rs                     # MODIFIED — warm_up ORDER BY ts, id
  eod/
    noise.rs                               # NEW — synthesize_session + inject_noise
    runner.rs                              # MODIFIED — call inject_noise after report,
                                           # before events delete
  domain/event.rs                          # unchanged — events serialize same as today

packages/shared/src/schemas/
  *.ts                                     # MODIFIED — event id types: z.string().uuid()
                                           # wherever currently typed as number

apps/cashier/src-tauri/Cargo.toml         # add `uuid = { version = "1", features = ["v4"] }`
                                           # and `rand = "0.8"` if not already present
```

Decisions:
- **`id` stored as `TEXT`** (UUID hyphenated form, 36 chars). Debuggability matters more than the 20-byte/row savings BLOB would give.
- **Tiebreaker** for warm-up ordering: `(ts ASC, id ASC)`. Random id provides deterministic tiebreak per instance without leaking ms-collision causality.
- **Noise injects fake SESSIONS, not isolated events.** A session has a complete chain, indistinguishable from real on decrypt.
- **Fakes never go through CommandService** — written directly to events.db via a new `EventStore::append_raw_with_id` method that takes pre-encrypted bytes + caller-supplied UUID + ts. They never affect AggregateStore (which they never need to, since the next bulk-delete clears them).
- **Plaintext content of fakes is generated from REAL catalog** (spots, staff, products) so decrypted fakes reference real entities. A fake "OrderPlaced" for a real spot with a real product looks identical to a real one.
- **Volume:** randomized 0.5x to 5x the count of real sessions for the day, with floor of 5 sessions if real count is small. Bounded so a quiet day doesn't produce zero noise.
- **Pre-prod migration:** drop and recreate `event` table; no data preservation. Same pattern as Plan F's destructive migrations.

---

## Task 1: UUID schema migration + EventStore type changes

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/events/0004_event_id_uuid.sql`
- Modify: `apps/cashier/src-tauri/src/store/events.rs`
- Modify: `apps/cashier/src-tauri/Cargo.toml` (add `uuid` v4)

- [ ] **Step 1: migration**

```sql
-- 0004_event_id_uuid.sql
DROP TABLE IF EXISTS event;

CREATE TABLE event (
  id                  TEXT PRIMARY KEY,
  business_day        TEXT NOT NULL,
  ts                  INTEGER NOT NULL,
  type                TEXT NOT NULL,
  aggregate_id        TEXT NOT NULL,
  actor_staff         INTEGER,
  actor_name          TEXT,
  override_staff_id   INTEGER,
  override_staff_name TEXT,
  payload_enc         BLOB NOT NULL,
  key_id              TEXT NOT NULL
);

CREATE INDEX idx_event_day_ts    ON event(business_day, ts, id);
CREATE INDEX idx_event_agg_ts    ON event(aggregate_id, ts, id);
CREATE INDEX idx_event_day_type  ON event(business_day, type);
```

(Three indexes: warm-up by day in causal order; per-aggregate replay; report-build filter.)

- [ ] **Step 2: events.rs type updates**

Change `EventRow.id: i64` → `EventRow.id: Uuid`. Same for `AppendEvent` if it carried id (it didn't — generated by autoincrement). Now `append` generates a fresh `Uuid::new_v4()` and inserts:

```rust
pub fn append(&self, ev: AppendEvent<'_>) -> AppResult<Uuid> {
    let id = Uuid::new_v4();
    self.with_writer(|conn| {
        conn.execute(
            "INSERT INTO event (id, business_day, ts, type, aggregate_id, actor_staff, actor_name,
                                override_staff_id, override_staff_name, payload_enc, key_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id.to_string(), ev.business_day, ev.ts, ev.event_type, ev.aggregate_id,
                ev.actor_staff, ev.actor_name, ev.override_staff_id, ev.override_staff_name,
                ev.payload_enc, ev.key_id
            ],
        )?;
        Ok(())
    })?;
    Ok(id)
}
```

Add `append_raw_with_id` for noise (Task 3 will use it):
```rust
pub fn append_raw_with_id(&self, id: Uuid, ev: AppendEvent<'_>) -> AppResult<()> {
    self.with_writer(|conn| {
        conn.execute(
            "INSERT INTO event (id, business_day, ts, type, aggregate_id, actor_staff, actor_name,
                                override_staff_id, override_staff_name, payload_enc, key_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                id.to_string(), ev.business_day, ev.ts, ev.event_type, ev.aggregate_id,
                ev.actor_staff, ev.actor_name, ev.override_staff_id, ev.override_staff_name,
                ev.payload_enc, ev.key_id
            ],
        )?;
        Ok(())
    })
}
```

All `SELECT … FROM event ORDER BY id` → `ORDER BY ts ASC, id ASC`. All `id INTEGER` parsing → `id TEXT` parse to `Uuid::parse_str(...)`.

- [ ] **Step 3: Cargo.toml**

```toml
uuid = { version = "1", features = ["v4", "serde"] }
```

(`rand` likely already a transitive dep via `aes-gcm`; add explicit `rand = "0.8"` if not.)

- [ ] **Step 4: tests**

Update existing event_store tests:
- `append_returns_uuid_increments_count` (was `append_returns_id_starting_at_1`)
- `list_orders_by_ts_then_id` (was `list_orders_by_id_asc`) — seed events with deliberate same-ts pairs and verify deterministic ordering

Helper `assert_event_id_is_uuid(row)` for tests that previously checked numeric id.

- [ ] **Step 5: cargo test events**

Expected: events tests green. Cross-cutting failures expected in callers — fix in Task 2.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(events): UUID v4 for event.id; ORDER BY (ts, id) for replay"
```

---

## Task 2: callers and warm-up — UUID type sweep

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/aggregate_store.rs` (warm_up)
- Modify: `apps/cashier/src-tauri/src/services/event_service.rs` (return type)
- Modify: `apps/cashier/src-tauri/src/services/command_service.rs` (caller of write)
- Modify: `apps/cashier/src-tauri/src/eod/builder.rs` (event iteration)
- Modify: `apps/cashier/src-tauri/src/http/routes/*` (any handler returning event_id)
- Modify: `packages/shared/src/schemas/*.ts` (zod: event id is `z.string().uuid()`)

- [ ] **Step 1: aggregate_store warm-up**

`SELECT … FROM event ORDER BY ts ASC, id ASC` (was `ORDER BY id`). EventRow.id is now Uuid; downstream apply() doesn't care about id type — it operates on payload.

- [ ] **Step 2: event_service**

`write` returns `Uuid` (was `i64`). Cascade through CommandService. Most internal callers don't care about the value; HTTP responses do.

- [ ] **Step 3: HTTP responses**

Any endpoint returning `{ event_id: number }` becomes `{ event_id: string }`. Search routes for `event_id` in JSON.

- [ ] **Step 4: zod schemas**

```ts
event_id: z.string().uuid()
```

Verify cashier + admin + web frontends still typecheck after the wire change.

- [ ] **Step 5: cargo test --lib && cargo test --tests**

Expected: 219 → 219 (no count change; type-only refactor). Any test asserting `id == 1` rewritten to assert UUID validity.

- [ ] **Step 6: pnpm typecheck across workspaces**

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor: UUID event ids — sweep callers, warm-up, HTTP wire, zod schemas"
```

---

## Task 3: fake-session generator

**Files:**
- Create: `apps/cashier/src-tauri/src/eod/noise.rs`
- Modify: `apps/cashier/src-tauri/src/eod/mod.rs` (add `pub mod noise;`)
- Modify: `apps/cashier/src-tauri/Cargo.toml` (add `rand = "0.8"` if not present)

- [ ] **Step 1: tests first**

```rust
// eod/noise.rs
#[cfg(test)]
mod tests {
    use super::*;
    use rand::SeedableRng;
    use rand_chacha::ChaCha8Rng;

    #[test]
    fn synthesize_session_produces_full_chain() {
        let spots = vec![Spot { id: 1, name: "Room 1".into(), kind: SpotKind::Room, ..Default::default() }];
        let products = vec![Product { id: 1, name: "Beer".into(), price: 50_000, ..Default::default() }];
        let staff = vec![Staff { id: 1, name: "Hoa".into(), role: Role::Cashier, ..Default::default() }];
        let mut rng = ChaCha8Rng::seed_from_u64(42);
        let day_start = 1_700_000_000_000_i64;
        let day_end = day_start + 24 * 3600 * 1000;

        let chain = synthesize_session(&spots, &products, &staff, day_start, day_end, &mut rng);

        // Must have at least: SessionOpened, ≥1 OrderPlaced, PaymentTaken, SessionClosed
        assert!(matches!(chain.first(), Some((_, DomainEvent::SessionOpened { .. }))));
        assert!(matches!(chain.last(), Some((_, DomainEvent::SessionClosed { .. }))));
        assert!(chain.iter().any(|(_, ev)| matches!(ev, DomainEvent::OrderPlaced { .. })));
        assert!(chain.iter().any(|(_, ev)| matches!(ev, DomainEvent::PaymentTaken { .. })));

        // All ts within day window, monotonically non-decreasing
        let mut last_ts = day_start;
        for (ts, _) in &chain {
            assert!(*ts >= day_start && *ts <= day_end);
            assert!(*ts >= last_ts);
            last_ts = *ts;
        }

        // All events share the same aggregate_id (the session id)
        let session_ids: std::collections::HashSet<_> = chain.iter().map(|(_, ev)| ev.aggregate_id()).collect();
        assert_eq!(session_ids.len(), 1);
    }

    #[test]
    fn synthesize_session_uses_real_catalog_entries() {
        // Verify spot/product/staff names in fake events are from the supplied catalog
        // (not random strings)
    }

    #[test]
    fn determine_noise_volume_floors_at_5_sessions() {
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        for real_count in 0..3 {
            let n = determine_noise_volume(real_count, &mut rng);
            assert!(n >= 5, "real={real_count}, got {n}");
        }
    }

    #[test]
    fn determine_noise_volume_scales_with_real_count() {
        let mut rng = ChaCha8Rng::seed_from_u64(0);
        for _ in 0..100 {
            let n = determine_noise_volume(20, &mut rng);
            assert!(n >= 10 && n <= 100, "n={n} outside 0.5x..5x of 20");
        }
    }
}
```

- [ ] **Step 2: implementation**

```rust
// eod/noise.rs
use crate::domain::event::DomainEvent;
use crate::domain::spot::SpotRef;
use rand::Rng;
use uuid::Uuid;

pub fn synthesize_session(
    spots: &[SpotRow],
    products: &[ProductRow],
    staff: &[StaffRow],
    day_start_ms: i64,
    day_end_ms: i64,
    rng: &mut impl Rng,
) -> Vec<(i64, DomainEvent)> {
    let session_id = Uuid::new_v4().to_string();
    let spot = pick(spots, rng);
    let staff_member = pick(staff, rng);

    let open_ts = rng.gen_range(day_start_ms..day_end_ms.saturating_sub(15 * 60_000));
    let duration_min = rng.gen_range(15..240);
    let close_ts = (open_ts + duration_min as i64 * 60_000).min(day_end_ms);

    let spot_ref = match spot.kind.as_str() {
        "room" => SpotRef::Room { id: spot.id, name: spot.name.clone(), hourly_rate: spot.hourly_rate.unwrap_or(0) },
        _ => SpotRef::Table { id: spot.id, name: spot.name.clone() },
    };

    let mut chain = vec![
        (open_ts, DomainEvent::SessionOpened {
            session_id: session_id.clone(),
            spot: spot_ref,
            opened_by_staff_id: staff_member.id,
            opened_by_staff_name: staff_member.name.clone(),
            opened_at_ms: open_ts,
        }),
    ];

    // 1-10 orders distributed across session
    let order_count = rng.gen_range(1..=10);
    for i in 0..order_count {
        let ts = open_ts + (close_ts - open_ts) * (i + 1) as i64 / (order_count + 1) as i64;
        let item_count = rng.gen_range(1..=5usize);
        let items = (0..item_count).map(|_| {
            let p = pick(products, rng);
            order_item_from_product(p, rng)
        }).collect();
        chain.push((ts, DomainEvent::OrderPlaced {
            session_id: session_id.clone(),
            order_id: Uuid::new_v4().to_string(),
            items,
            placed_by_staff_id: staff_member.id,
            placed_by_staff_name: staff_member.name.clone(),
            placed_at_ms: ts,
        }));
    }

    // payment + close
    let pay_ts = close_ts - 60_000;
    let total = chain.iter().filter_map(|(_, ev)| order_total(ev)).sum::<i64>();
    chain.push((pay_ts, DomainEvent::PaymentTaken {
        session_id: session_id.clone(),
        subtotal: total,
        discount_pct: 0,
        vat_pct: 0,
        total,
        method: pick_method(rng),
        taken_by_staff_id: staff_member.id,
        taken_by_staff_name: staff_member.name.clone(),
        taken_at_ms: pay_ts,
    }));
    chain.push((close_ts, DomainEvent::SessionClosed {
        session_id: session_id.clone(),
        closed_at_ms: close_ts,
    }));

    chain
}

pub fn determine_noise_volume(real_session_count: usize, rng: &mut impl Rng) -> usize {
    let scaled = if real_session_count == 0 { 0 } else {
        let lo = (real_session_count as f64 * 0.5).round() as usize;
        let hi = (real_session_count as f64 * 5.0).round() as usize;
        rng.gen_range(lo..=hi.max(lo + 1))
    };
    scaled.max(5)
}

fn pick<'a, T>(slice: &'a [T], rng: &mut impl Rng) -> &'a T {
    &slice[rng.gen_range(0..slice.len())]
}
```

(Adapt field names to actual `DomainEvent` variants in `domain/event.rs`. Helpers like `order_total`, `order_item_from_product`, `pick_method` are small.)

- [ ] **Step 3: cargo test eod::noise**

Expected: 4/4 pass.

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(eod): fake-session generator (full causally-valid chains)"
```

---

## Task 4: noise injection in EOD runner

**Files:**
- Modify: `apps/cashier/src-tauri/src/eod/runner.rs`
- Modify: `apps/cashier/src-tauri/src/store/master.rs` (add `list_spots/list_products/list_staff` if missing — admin already has versions, may need exposing)

- [ ] **Step 1: business-day window helpers**

In `eod/business_day.rs`, add:
```rust
pub fn day_window_ms(business_day: &str, cfg: Cfg) -> (i64, i64) {
    // Start of business day = (parse YYYY-MM-DD) at cutoff_hour local time
    // End = start + 24h
    let date = NaiveDate::parse_from_str(business_day, "%Y-%m-%d").unwrap();
    let local_start = date.and_hms_opt(cfg.cutoff_hour, 0, 0).unwrap();
    let tz = FixedOffset::east_opt(cfg.tz_offset_seconds).unwrap();
    let start_ms = tz.from_local_datetime(&local_start).unwrap().with_timezone(&Utc).timestamp_millis();
    (start_ms, start_ms + 24 * 3600 * 1000)
}
```

Test: `day_window_ms("2026-04-27", default_cfg) == (start_of_business_day_in_utc_ms, +24h)`.

- [ ] **Step 2: runner change**

In `run_eod`, after the master tx commits and BEFORE `events.delete_day`:

```rust
// 6.5) Noise injection — fake sessions to muddy free-page recovery.
//      Generated post-report (so report content is unaffected) and pre-delete
//      (so fake rows immediately become free pages alongside real ones).
let (day_start, day_end) = day_window_ms(business_day, cfg);
let real_session_count = count_real_sessions(state, business_day)?;
let noise_count = noise::determine_noise_volume(real_session_count, &mut rng);
let spots = state.master.lock().unwrap().list_spots()?;
let products = state.master.lock().unwrap().list_products()?;
let staff = state.master.lock().unwrap().list_staff()?;
let dek = state.key_manager.dek_for(&utc_day_for(day_start))?;
let mut written = 0;
for _ in 0..noise_count {
    let chain = noise::synthesize_session(&spots, &products, &staff, day_start, day_end, &mut rng);
    for (ts, ev) in chain {
        let payload = serde_json::to_vec(&ev).unwrap();
        let utc_day = utc_day_of(ts);
        let aad = format!("{business_day}|{}|{}|{utc_day}", ev.event_type().as_str(), ev.aggregate_id());
        let blob = dek.encrypt(&payload, aad.as_bytes())?;
        state.events.append_raw_with_id(Uuid::new_v4(), AppendEvent {
            business_day,
            ts,
            event_type: ev.event_type().as_str(),
            aggregate_id: &ev.aggregate_id(),
            actor_staff: None,
            actor_name: None,
            override_staff_id: None,
            override_staff_name: None,
            payload_enc: &blob,
            key_id: &utc_day,
        })?;
        written += 1;
    }
}
tracing::info!(business_day, sessions = noise_count, events = written, "eod noise injected");

// 7) Delete (real + fake) — same as before
```

(Subtle: events that span the UTC-day boundary use the DEK for the day they fall on. For simplicity, generate all noise within a single UTC day matching the start of the business day. Slightly simpler than per-event DEK lookup; minor model fidelity loss is acceptable for noise.)

- [ ] **Step 3: integration tests**

```rust
#[tokio::test]
async fn eod_injects_noise_then_deletes_all_for_day() {
    let state = seed_app_state_with_clock(ts_for("2026-04-27 12:00 +07")).await;
    place_test_order(&state, ts_for("2026-04-27 14:00 +07")).await;
    take_test_payment(&state, ts_for("2026-04-27 14:30 +07")).await;
    let pre_count = event_count(&state).await;

    run_eod(&state, "2026-04-27").await.unwrap();

    // After run: events for that day are gone (real + fake)
    let post_count = event_count_for_day(&state, "2026-04-27").await;
    assert_eq!(post_count, 0);

    // Pre-count was real only; during EOD we wrote real + noise then deleted both;
    // we don't have a hook to observe noise count post-hoc — just verify report
    // matches real (no noise leaked into report)
    let report = read_report(&state, "2026-04-27").await;
    assert_eq!(report.orders.len(), 1);
    assert_eq!(report.payments.len(), 1);
}

#[tokio::test]
async fn eod_noise_does_not_affect_aggregate_state() {
    // Run EOD with noise; verify AggregateStore active sessions unchanged
}
```

- [ ] **Step 4: cargo test eod**

Expected: all eod tests green. Total cargo test count: 219 + ~6 new = ~225.

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(eod): inject fake-session noise before bulk-delete (forensic privacy)"
```

---

## Task 5: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — count grows from 219 to ~225
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] All workspace typechecks: shared, pos-ui, cashier, web, admin
- [ ] Manual smoke: run `cashier eod-now <day>`, verify daily_report contents match real events only (no noise polluted the report), verify event row count for that day = 0 post-run

---

## Done

End state:
- `event.id` is UUID v4; ordering by `(ts, id)`. Free-page metadata recovery yields opaque random IDs.
- EOD runner injects randomized count (≥5, 0.5x..5x of real session count) of fake sessions before bulk-deleting that business day's rows. Each fake is a complete causally-valid chain (open → orders → payment → close) with random spot/staff/products from the real catalog and timestamps spread across the business day.
- Free pages on the SSD now contain a mix of real + fake ciphertext. Within the 4-day DEK-retention window, an attacker who recovers a wrapped DEK and pages and has KEK access decrypts both kinds and cannot distinguish them by id, ts, content shape, or aggregate causality.
- Reports remain real-only (noise generated post-report-write).
- Aggregate state is unaffected (noise bypasses CommandService and is deleted within the same EOD step).
- Forensic carving past the 4-day window: ciphertext (real or fake) is unreadable because the DEK is gone via rotation.

Out of scope, separately tracked:
- Reports remain plaintext on disk — biggest historical-data leak surface (separate spec)
- KEK lives in OS keychain — pluggable KekSource (manager PIN / WebAuthn) is a separate plan
- SQLCipher whole-DB encryption — separate plan
