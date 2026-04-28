# Foundation Design — Karaoke/Bar POS

**Date:** 2026-04-27
**Subsystem:** 1 of 4 (Foundation)
**Status:** Draft for review

## Purpose

Establish the core platform that subsequent subsystems (cashier UI, order tablet, inventory) build on. Foundation owns: process model, persistence, encryption, event store, daily key rotation, LAN HTTP/WS server, auth, end-of-day pipeline, print stub.

## Scope

**In:**
- Tauri app shell (Rust core + React UI scaffold)
- Embedded HTTP+WS server on LAN
- SQLite persistence (master CRUD + append-only event store)
- Column-level AES-GCM encryption with daily key rotation
- Key storage in OS keychain
- Staff PIN auth
- End-of-day pipeline (event fold → reports → shred)
- `print(type, data)` stub
- Migration runner

**Out (later subsystems):**
- Cashier UI screens, order tablet UI, recipe editor, inventory consumer, real printer drivers, file saving for reports, settings UI for printer mapping.

## Non-Goals

- Customer self-order (QR menu)
- Multi-master replication or cloud sync
- Whole-DB encryption (SQLCipher); we use column-level instead
- Persistent file output for reports (deferred)

## Domain Model

### Aggregates (event-sourced)

- **Session** — a room or table opened for a customer. Identified by `session_id`. Cross-midnight allowed; session belongs to its **opening business day**.
- **Order** — a set of items added to a session. One session has many orders.
- **Payment** — settles a session.

### Master entities (CRUD, not event-sourced)

- `staff` (id, name, pin_hash, role, team)
- `room` (id, name, hourly_rate, status)
- `table` (id, name, room_id?, status)
- `product` (id, name, price, route, kind) — `route` ∈ {kitchen, bar, none}; `kind` ∈ {item, recipe, time}
- `recipe` (product_id, ingredient_id, qty_grams_or_units) — used at EOD to derive inventory deltas
- `setting` (key, value)
- `day_key` (business_day, wrapped_dek, created_at) — wrapped per-day DEKs; row deletion = crypto-shred
- `daily_report` (business_day PK, generated_at, order_summary_json, inventory_summary_json)

### Event types (transactional, encrypted)

| Event | Aggregate | Payload (decrypted) |
|---|---|---|
| `SessionOpened` | session | `{room_id?, table_id?, opened_by, customer_label, team}` |
| `SessionClosed` | session | `{closed_by, reason}` |
| `SessionTransferred` | session | `{from, to}` (room↔room, table↔table, table→room) |
| `SessionMerged` | session | `{into_session, sources[]}` |
| `SessionSplit` | session | `{from_session, new_sessions[]}` |
| `OrderPlaced` | order | `{session_id, items: [{product_id, qty, unit_price, note}]}` |
| `OrderItemCancelled` | order | `{order_id, item_index, reason}` |
| `OrderItemReturned` | order | `{order_id, item_index, qty, reason}` |
| `PaymentTaken` | payment | `{session_id, subtotal, discount_pct, vat_pct, total, method}` |

Events are append-only. Current state = projection over events.

### Daily report (output of EOD)

- **Order report:** gross, discount, vat, net; per-product qty+revenue; per-room hours+revenue; per-payment-method total; per-staff orders+sales; counts of voided/returned items.
- **Inventory report:** per-ingredient consumed qty (sum over OrderPlaced × recipe), per-product void qty.
- No PII, no per-session detail.

## Architecture

```
┌─────────────────────────────────────────────────────┐
│  Tauri app (cashier machine)                        │
│  ┌───────────────────────────────────────────────┐  │
│  │ React UI (cashier shell)                      │  │
│  └──────────────────┬────────────────────────────┘  │
│                     │ tauri.invoke                  │
│  ┌──────────────────▼────────────────────────────┐  │
│  │ Rust core (single process)                    │  │
│  │  ┌─────────────┐ ┌──────────────────────┐     │  │
│  │  │ Domain svcs │ │ axum HTTP+WS server  │◄────┼──── LAN
│  │  └──────┬──────┘ └──────────┬───────────┘     │  │      │
│  │         │                   │                  │  │      │
│  │  ┌──────▼───────────────────▼──────────┐      │  │      │
│  │  │ Storage (rusqlite)                  │      │  │      │
│  │  │  - master.db (CRUD)                 │      │  │      │
│  │  │  - events.db (append-only, encrypt) │      │  │      │
│  │  └─────────────────────────────────────┘      │  │      │
│  │  ┌─────────────────────────────────────┐      │  │      │
│  │  │ Crypto (AES-256-GCM, daily key)     │      │  │      │
│  │  │ Keychain (keyring-rs)               │      │  │      │
│  │  └─────────────────────────────────────┘      │  │      │
│  │  ┌─────────────────────────────────────┐      │  │      │
│  │  │ EOD scheduler + print(type, data)   │      │  │      │
│  │  └─────────────────────────────────────┘      │  │      │
│  └───────────────────────────────────────────────┘  │      │
└─────────────────────────────────────────────────────┘      │
                                                             │
                ┌────────────────────────────────────────────┘
                ▼
        ┌──────────────────────┐
        │ Order tablet         │  apps/web (Vite+React+shadcn)
        │ - PIN login          │  PWA, fetch + WS to cashier IP
        └──────────────────────┘
```

### Process model

Single Tauri process. No sidecar. axum runs on a Tokio runtime inside Tauri's main Rust process, bound to `0.0.0.0:<port>` (configurable, default 7878). Cashier UI calls Rust via `tauri::command`. Tablets call same domain services via HTTP/WS. Both paths share identical service layer — HTTP/WS handlers and Tauri commands are thin wrappers.

### Storage

Two SQLite files, both via `rusqlite`:

**`master.db`** — CRUD tables (staff, room, table, product, recipe, setting, daily_report_index). Plaintext on disk. Migrations via embedded SQL files + a `_migrations` table tracker.

**`events.db`** — single table:
```sql
CREATE TABLE event (
  id            INTEGER PRIMARY KEY AUTOINCREMENT,
  business_day  TEXT NOT NULL,           -- 'YYYY-MM-DD' (plaintext, indexed for shred)
  ts            INTEGER NOT NULL,        -- unix ms (plaintext, for ordering)
  type          TEXT NOT NULL,           -- event class (plaintext, for filtered fold)
  aggregate_id  TEXT NOT NULL,           -- session_id / order_id (plaintext, for projections)
  actor_staff   INTEGER,                 -- plaintext FK
  payload_enc   BLOB NOT NULL,           -- AES-GCM(nonce(12) || ciphertext || tag(16))
  key_id        TEXT NOT NULL            -- 'YYYY-MM-DD' — which day key encrypted this
);
CREATE INDEX idx_event_day ON event(business_day);
CREATE INDEX idx_event_agg ON event(aggregate_id, id);
```

`payload_enc` holds the JSON-serialized event body, encrypted. Indexable fields stay plaintext (business_day for shred, type/aggregate_id for projections, ts for ordering).

### Reports

`master.db`:
```sql
CREATE TABLE daily_report (
  business_day TEXT PRIMARY KEY,
  generated_at INTEGER NOT NULL,
  order_summary_json TEXT NOT NULL,    -- aggregated, no PII
  inventory_summary_json TEXT NOT NULL
);
```
For now we also `print('eod-order-report', summary)` and `print('eod-inventory-report', summary)` via the stub. File persistence is deferred to a later subsystem.

### Encryption

- **Algorithm:** AES-256-GCM, per-event random 96-bit nonce.
- **Two-tier key model:**
  - **KEK** (Key Encryption Key) = 32 random bytes generated on first launch, stored in OS keychain as `<app-id>.master`. Never rotates. Never used to encrypt events directly.
  - **DEK** (Data Encryption Key) = 32 random bytes generated per business day. Used to encrypt that day's event payloads. Wrapped with KEK (AES-GCM) and stored in `master.db.day_key(business_day, wrapped_dek, created_at)`. Held in memory while the day is active.
- **Crypto-shred:** at EOD, after report is generated and verified persisted, **delete the row from `day_key` for that business day**. The wrapped DEK is gone from disk; the in-memory DEK is zeroed. With no DEK and no way to re-derive it from KEK, all events for that day become permanently unreadable. Then `DELETE FROM event WHERE business_day = ?` and `VACUUM`. Defense in depth: even if a forensic read recovered ciphertext bytes from freed pages, no key exists to decrypt them.
- **What's encrypted:** event payloads only. Indexed columns and master tables are plaintext.

Why random DEK instead of HKDF-derived: an HKDF-derived key is recoverable as long as the KEK survives. Crypto-shred requires the key material itself be unrecoverable. Random DEK + delete-the-wrapped-copy achieves that.

Encryption helper API:
```rust
pub fn enc(day: &str, payload: &[u8]) -> Vec<u8>;   // -> nonce||ct||tag
pub fn dec(day: &str, blob: &[u8]) -> Result<Vec<u8>>;
```

### Key storage

- Backend: `keyring-rs` (cross-platform: macOS Keychain, Windows DPAPI, Linux Secret Service).
- Namespace: `com.lofi-pos.master`.
- First-run flow: generate 32 random bytes → store in keychain → done.
- Subsequent runs: read once at startup → keep in memory for daily key derivation.
- Setting flag (future): "require passphrase on launch" → wrap master entropy with Argon2id-derived KEK; prompt at start instead of keychain auto-unlock. Out of scope now.

### Auth

Everyone authenticates. No anonymous access — cashier UI included. Every action carries a known actor for ACL + audit.

- **PIN login** — `POST /auth/login {pin}` → server compares against `staff.pin_hash` (Argon2id) → returns signed token (HMAC over `staff_id+role+expiry`, signing key in keychain).
- **Order tablet** — token stored in HttpOnly cookie. All HTTP/WS endpoints check cookie. Session 12h.
- **Cashier UI (Tauri)** — same PIN login screen on launch. Token kept in Rust process memory keyed by Tauri window; every `tauri::command` requires an actor in scope. Lock screen on idle (configurable, default 10 min).
- **Sign-out** — `POST /auth/logout` invalidates server-side session id; cashier shows lock screen.

### Authorization (ACL)

Role-based with **supervisor override** for sensitive actions (manager swipes PIN inline). Both HTTP middleware and Tauri command wrapper run the same policy check.

#### Roles

`staff.role` enum:
- `staff` — order entry, open/close sessions
- `cashier` — staff + take payment, apply small discount
- `manager` — cashier + void/return, transfer/merge/split, large discount, edit menu, view live reports
- `owner` — manager + EOD, edit staff/settings/recipes

#### Permission matrix

| Action | staff | cashier | manager | owner |
|---|---|---|---|---|
| Login | ✓ | ✓ | ✓ | ✓ |
| Open session, add order | ✓ | ✓ | ✓ | ✓ |
| Cancel item (within `cancel_grace_minutes`, qty 1, by self) | ✓ | ✓ | ✓ | ✓ |
| Cancel item (general) | override | override | ✓ | ✓ |
| Return item | override | override | ✓ | ✓ |
| Discount ≤ `discount_threshold_pct` | override | ✓ | ✓ | ✓ |
| Discount > threshold | override | override | ✓ | ✓ |
| Transfer / merge / split | override | override | ✓ | ✓ |
| Take payment | — | ✓ | ✓ | ✓ |
| View live reports | — | — | ✓ | ✓ |
| Run EOD | — | — | — | ✓ |
| Edit menu/products | — | — | ✓ | ✓ |
| Edit recipes | — | — | — | ✓ |
| Edit staff/settings | — | — | — | ✓ |

`override` = action allowed if a valid `X-Override-PIN` (HTTP) or `override_pin` arg (Tauri command) for a staff with role ≥ required is supplied with the request.

#### Mechanics

```rust
pub enum Decision { Allow, Deny, OverrideRequired(Role) }
pub fn check(action: Action, actor: &Staff, ctx: &PolicyCtx) -> Decision;
```

- Deny → HTTP 403 `{code: "forbidden"}`.
- OverrideRequired with no/invalid override → HTTP 403 `{code: "override_required", min_role: "manager"}`. Client UI prompts and retries with header.
- All event payloads include `actor_staff` and (when present) `override_staff`. Daily report aggregates override counts per staff.

#### Settings (in `setting` table)

- `discount_threshold_pct` (default 10)
- `cancel_grace_minutes` (default 5)
- `idle_lock_minutes` (default 10)
- `business_day_cutoff_hour` (default 11)

### Day boundary & EOD

- **Cutoff:** configurable via `setting.business_day_cutoff_hour`, default **11:00** (local TZ).
- **Business day for an event:** computed from event `ts` rolled back by cutoff hour (e.g. 2026-04-28T03:00 with cutoff 11 → business_day `2026-04-27`).
- **Open sessions across cutoff:** session belongs to its opening business day; subsequent events on that session continue to be tagged with the opening business day until SessionClosed.
- **EOD trigger:** manual button in cashier UI, with confirmation. Auto-trigger optional later.
- **EOD pipeline:**
  1. Compute target business day = previous calendar business day.
  2. Block if any open sessions for that day (force-close prompt lists them).
  3. Stream events for that day, decrypt, fold into projections.
  4. Build order summary + inventory summary (inventory = sum of recipe-expanded ingredient consumption from OrderPlaced minus voids).
  5. Insert into `daily_report`.
  6. `print('eod-order-report', order_summary)` + `print('eod-inventory-report', inventory_summary)`.
  7. `DELETE FROM day_key WHERE business_day = ?` and zero in-memory DEK. **Events for that day are now permanently unreadable.**
  8. `DELETE FROM event WHERE business_day = ?`.
  9. `VACUUM` events.db (reclaim space + overwrite freed pages).

### Print stub

```rust
#[derive(Debug, Serialize)]
pub enum PrintType {
    EodOrderReport, EodInventoryReport,
    KitchenTicket, BarTicket, CashierBill,
}

pub fn print(ticket_type: PrintType, data: serde_json::Value) -> Result<()> {
    tracing::info!(?ticket_type, ?data, "print stub");
    Ok(())
}
```

Settings-driven type→device mapping deferred.

## API Surface

All HTTP endpoints mirror existing Tauri commands; service layer is shared. WS endpoint `GET /ws` for state push.

### Auth
- `POST /auth/login` `{pin}` → 200 + cookie / 401
- `POST /auth/logout`

### Read
- `GET /rooms`, `GET /tables`, `GET /products`
- `GET /sessions/active`
- `GET /sessions/:id` (returns projection)

### Write (commands → emit events)
- `POST /sessions` `{room_id|table_id, customer_label, team}` → opens
- `POST /sessions/:id/close`
- `POST /sessions/:id/transfer` `{to_room_id|to_table_id}`
- `POST /sessions/merge` `{into, sources[]}`
- `POST /sessions/:id/split` `{new_sessions:[{items[]}]}`
- `POST /orders` `{session_id, items[]}`
- `POST /order-items/:order_id/:idx/cancel` `{reason}`
- `POST /order-items/:order_id/:idx/return` `{qty, reason}`
- `POST /sessions/:id/payment` `{discount_pct, vat_pct, method}`

### EOD
- `POST /admin/eod` `{business_day}` (cashier-UI only — gated behind Tauri command, not HTTP)

### WS events
- `event.appended`, `session.updated`, `order.updated`

## Repo Layout

```
apps/
  cashier/                  # NEW — Tauri app
    src/                    # React UI (cashier shell scaffold only)
    src-tauri/
      src/
        main.rs             # Tauri entry, axum spawn
        crypto.rs           # AES-GCM + HKDF + keychain
        store/
          master.rs         # CRUD tables
          events.rs         # event store, fold helpers
          migrations/*.sql
        domain/
          session.rs
          order.rs
          payment.rs
        api/
          http.rs           # axum routes
          ws.rs
          tauri_cmd.rs      # tauri::command bridge
        eod.rs
        print.rs
        auth.rs
      Cargo.toml
      tauri.conf.json
  web/                      # existing — repurposed as order tablet
packages/
  ui/                       # existing shadcn
  shared/                   # NEW — TS types + zod + fetch/WS client
docs/superpowers/specs/
```

## Error Handling

- Rust: `thiserror` enum `AppError { Db, Crypto, NotFound, Conflict, Unauthorized, Validation, Internal }`.
- HTTP: errors map to `{code, message}` JSON with appropriate status (400/401/404/409/500).
- Tauri commands: return `Result<T, String>` (serialized error code).
- All event writes inside transactions; failures roll back.
- WS clients reconnect with exponential backoff (cap 30s).
- EOD pipeline is idempotent: `daily_report` insert is `INSERT OR IGNORE`; row delete is gated on report existence.

## Testing

Test pyramid: most tests are Rust unit tests against pure functions and in-memory SQLite. A smaller integration tier exercises the axum server and the Tauri command bridge against a real (temp-dir) DB. A thin E2E tier drives a tablet client against a running cashier process. UI rendering is out of scope for foundation tests; those land with subsystem 2.

### Layer 1 — Rust unit (fast, in-memory)

Each module ships with focused unit tests.

- **`crypto`**
  - AES-GCM round-trip: enc → dec returns plaintext; tampering ciphertext or AAD fails.
  - Nonce uniqueness over N=10k encryptions (statistical sanity).
  - Wrap/unwrap: KEK wraps random DEK; unwrap with wrong KEK fails.
  - Zeroization: `Drop` on key wrappers overwrites memory (verify via `zeroize::Zeroize`).
- **`store::events`**
  - Append + read by aggregate returns events in id order.
  - Append is transactional (failed write rolls back; sequence id not consumed).
  - Filter by `business_day` returns only that day.
  - Each event row carries the correct `key_id`.
- **`store::master`**
  - Migration runner applies SQL files in order, idempotent on re-run.
  - CRUD round-trips for each master table.
  - `day_key` insert/lookup/delete works; deletion is hard delete (no soft-delete).
- **`domain::session` / `order` / `payment`**
  - Projection fold: replay event sequence yields expected current state.
  - Invariants:
    - Cannot add order to a closed session.
    - Cannot pay a session twice.
    - Merge: target absorbs sources, sources marked merged-into, projection of merged session contains union of items.
    - Split: source items partitioned across new sessions, sum of new = source pre-split.
    - Transfer table↔room preserves order history.
    - Cross-midnight session keeps opening `business_day` on subsequent events.
- **`auth`**
  - Argon2id PIN hash + verify; constant-time compare.
  - Token sign + verify; tampered token rejected; expired token rejected.
- **`policy`** (ACL — table-driven, exhaustive)
  - One test per row of the permission matrix: `(action × role) → expected Decision`.
  - Override resolution: insufficient role + valid override-PIN of sufficient role → Allow; insufficient override → OverrideRequired.
  - `cancel_grace_minutes` boundary: at grace_minutes - 1s allow, at grace_minutes + 1s require override.
  - `discount_threshold_pct` boundary: at threshold allow for cashier, above require override.
- **`eod`** (the riskiest module — extra coverage)
  - Happy path: seed events for day D, run EOD, assert `daily_report` row exists with expected aggregates, `day_key` row gone, `event` rows for D gone, in-memory DEK zeroed, ciphertext rows for D undecryptable post-shred (assert `dec` returns `Err`).
  - Block when open sessions for D exist; lists offenders.
  - Idempotent: running EOD twice for same D is a no-op (second run sees report present, skips).
  - Crash mid-pipeline: simulate panic between report-write and key-delete; on restart the recovery path re-runs delete (resumable).
  - Inventory derivation: orders for products A (recipe: 2 of ingredient X) and B (recipe: 3 of X) with voids → expected ingredient totals match.
- **`business_day`**
  - Cutoff math: timestamps just before/after cutoff hour map to correct business day across DST transitions and TZ.

### Layer 2 — Rust integration (real SQLite, real axum)

Spin a fresh temp-dir DB and an axum server on an ephemeral port per test. Use `reqwest` from the test.

- **Auth flow:** login with valid/invalid PIN; cookie required on protected routes; logout invalidates token.
- **End-to-end happy path:** open session → add order → take payment → state visible via `GET /sessions/:id`.
- **ACL via HTTP:** each matrix row exercised once; override header path covered.
- **Concurrent writes:** two clients add orders to same session in parallel; both events appended, no lost writes, projection consistent. (Also covers Tokio + `spawn_blocking` plumbing.)
- **WS broadcast:** subscriber receives `event.appended` after a `POST /orders`.
- **Tauri command bridge:** invoke each `tauri::command` via the same service layer (no Tauri runtime needed — wrappers tested directly with mocked window/actor context). Confirms HTTP and Tauri share identical behavior.
- **Cookie + override interplay:** cashier's cookie + manager's override-PIN succeeds; manager's cookie alone (no override needed) succeeds.

### Layer 3 — End-to-end (one happy path)

Single smoke test: launch the cashier binary in headless mode (Tauri test harness or a `--server-only` flag), have a node script (or `reqwest` test) act as a tablet — login, open session, add items, pay, run EOD, assert report row + events shred. Runs in CI on Linux only (Tauri build is heavy on macOS/Windows runners; cross-platform smoke deferred).

### Property-based tests (where they pay off)

Use `proptest` for:
- **Projection determinism:** for any random-but-valid event sequence, replay yields the same projection regardless of read order grouping.
- **Crypto:** any 0–4KB plaintext + any 32-byte key round-trips.
- **Business-day math:** for any `(timestamp, cutoff_hour, tz)` the inverse mapping is consistent.

### Test data + helpers

- `testkit` crate inside `src-tauri/`: builders for `Staff`, `Session`, `OrderItem`; an `EventStream::seed()` helper; a `temp_db()` returning a configured store on a tempdir; a `clock()` injectable time source so `business_day` and `cancel_grace_minutes` are deterministic.
- All time-sensitive code takes `&dyn Clock`; production = `SystemClock`, tests = `MockClock`.

### Negative + security tests

- **Tamper:** flip one byte of `payload_enc`, expect `dec` to fail (GCM tag).
- **Wrong key:** decrypt event from day D using day D-1 DEK, expect failure.
- **Crypto-shred verification:** capture ciphertext + key_id pre-EOD; post-EOD, even with KEK still in keychain, the ciphertext must be undecryptable (DEK is gone).
- **Replay attack on token:** old token after logout rejected.
- **Permission bypass:** every protected endpoint hit without cookie → 401; with low-role cookie → 403/override_required.

### TypeScript

- `packages/shared`: vitest on zod schemas (parse fixtures of every event/command, assert pass/fail).
- `apps/web` (tablet client): vitest on the typed fetch+WS client (mock server with `msw`). UI component tests deferred to subsystem 3.

### CI

- `cargo test --all` (unit + integration + proptest).
- `cargo clippy -- -D warnings`, `cargo fmt --check`.
- `pnpm typecheck`, `pnpm lint`, `pnpm test` (vitest).
- E2E job runs only on `main` and PRs touching `src-tauri/` or `apps/web/`.
- Coverage target (informational, not gating): `cargo llvm-cov` ≥ 80% on `domain`, `eod`, `policy`, `crypto`.

### What is NOT tested in foundation

- React UI rendering (no screens yet).
- Real printer drivers (stub only).
- File-saved reports (deferred).
- Cross-platform install/update (CI Linux only for foundation phase).

## Open Decisions (resolved)

- Runtime: **Tauri + Rust** ✓
- Encryption: **column-level AES-GCM with daily-derived key** (no SQLCipher) ✓
- Day cutoff: **11:00, configurable** ✓
- After EOD: **delete day's events + drop key from memory** (no archive) ✓
- Reports: **DB row + print stub only**; file save later ✓
- Inventory: **derived at EOD from order events × recipes; no inventory state in cashier** ✓
- Auth: **PIN for tablets, none for cashier UI** ✓
- Customer self-order: **out of scope** ✓

## Risks / Notes

- Tax retention: aggregated `daily_report` is the only durable transactional record. Confirm acceptable with accountant; raw per-session/per-customer data is unrecoverable post-EOD by design.
- Master entropy in keychain: if the OS user account is compromised while logged in, attacker can derive any day key for events still on disk (i.e., today's). Same threat model as the data itself — mitigated by EOD shred.
- `VACUUM` on events.db can be slow at high event counts; acceptable post-EOD when no traffic.
- Single-process axum + Tauri shares Tokio runtime — long-running handlers must not block. All DB ops use `spawn_blocking`.
