# `bouncer` Sidecar — Design

**Status:** Active. Replaces local UTC key rotation (`2026-04-28-utc-key-rotation-design.md`) and the UUID/noise plan (`2026-04-29-uuid-event-id-and-noise-sessions.md`).

**Goal:** Move all integration concerns (key custody, printers, off-box reports) out of the cashier into a separate local HTTP service called `bouncer`. Cashier becomes a pure UI + business-logic + event-store process; bouncer is the door-keeper between the cashier and everything outside it (peripherals, key material, off-box destinations).

The real `bouncer` will be built by another team. This spec defines the wire protocol they'll implement against, plus a mock `bouncer` we ship in this repo for development and tests.

---

## Why this design

Local rotation + KEK-in-keychain + local plaintext reports turned out to leak in too many places once the real POS environment was examined: weak Windows passwords defeat FDE, SSD wear-leveling defeats `secure_delete`, and reports were the biggest plaintext-on-disk surface anyway. The simpler answer is to stop trying to make the cashier secure on its own and just push the secret-handling and the off-box delivery to a separate process whose threat model is somebody else's spec.

What we keep on the cashier:
- The event store (encrypted)
- All business logic
- The UI

What moves to bouncer:
- Seeds (no more KEK in OS keychain, no more `dek` table, no more `daily_report` table, no more `reports/` directory)
- Printer routing (no more `print()` to stdout)
- Daily report delivery

Crypto-shred shifts from "cashier deletes its own key" to "bouncer removes a seed from its list." Cashier doesn't even know how bouncer stores its seeds; that's the bouncer team's spec.

---

## Wire protocol

`bouncer` is HTTP on `127.0.0.1:7879` (configurable via `LOFI_BOUNCER_URL` env). Localhost-only trust boundary, no app-layer auth in v1. Cashier is configured at startup with the URL and just talks to it.

### Seeds

```
GET /seeds → 200 OK
[
  {
    "id": "string",                      // opaque identifier, e.g. UUID or short slug
    "label": "string",                   // human label, e.g. "2026-Q2"
    "default": true | false,             // exactly one entry has default=true
    "seed_hex": "64-char hex string"     // 32 raw bytes, hex-encoded
  },
  ...
]
```

Order: caller does not depend on order. Exactly one seed has `default=true`. Seed bytes are returned in plaintext over loopback; bouncer's at-rest protection of seeds is its own concern.

### Printers

```
GET /printers → 200 OK
[
  { "id": "string", "name": "string", "kind": "kitchen" | "bar" | "receipt" | "report" }
]

POST /print
  body: {
    "kind": "order_ticket" | "receipt" | "session_closed" | "eod_summary",
    "payload": { ... arbitrary JSON ... },
    "target_printer_id": "string" | null   // optional override; bouncer routes by kind if null
  }
  → 202 Accepted   {"queued": true}
  → 503 Service Unavailable on transient failure
```

Cashier always treats `POST /print` as fire-and-forget logically (the response carries no data the cashier needs), but the HTTP call itself blocks the caller until response. The cashier wraps it in a background queue so callers (CommandService side-effects) don't block.

### Reports

```
POST /reports/eod
  body: {
    "business_day": "YYYY-MM-DD",
    "generated_at": <unix-ms>,
    "report": { ... full daily report JSON ... }
  }
  → 200 OK   {"stored": true}
```

EOD blocks on this call until success. If bouncer rejects (4xx) or is unreachable, EOD does NOT mark the day complete; next run retries.

### Health

```
GET /health → 200 OK   {"ok": true}
```

Cashier hits this once at startup. Failure → cashier refuses to start (hard-fail bootstrap).

---

## Cashier integration

### Bootstrap

1. App starts. Reads `LOFI_BOUNCER_URL` (default `http://127.0.0.1:7879`).
2. Calls `GET /health` (best-effort log only) then `GET /seeds`. Caches the full returned list in RAM, including which one is `default=true`.
3. **Cashier owns no fallback.** The bouncer (separate service) handles its own internal fallback and is contractually expected to always return at least one seed (with one marked `default=true`) when reachable. Whatever the bouncer returns is what the cashier uses; from the cashier's perspective there is no special "fallback seed" — every `seed_id` is opaque.
4. **Hard-fail at startup** if any of the following occurs:
   - bouncer unreachable (network error, timeout, non-200), OR
   - bouncer returns an empty seed list, OR
   - bouncer returns no seed marked `default=true`.

   In any of these cases the cashier exits with a non-zero status and an operator-friendly message (`bouncer not reachable; start bouncer service first`). There is no "degraded mode."
5. On success, proceeds to normal startup (axum, scheduler, etc.).

Seed cache is **never refreshed** during the process lifetime. To pick up a rotated default seed, restart the cashier.

### Key derivation

```
DEK = blake3(seed_bytes ++ business_day_bytes)   // 32-byte output
```

`business_day` is the existing cutoff-aware day string (`YYYY-MM-DD`). One DEK per (seed, business_day) pair. DEKs are derived on demand and held in process memory only.

### Encryption write path

```
seed       = cache.default()
business_day = business_day_of(now)
dek        = blake3(seed.bytes, business_day.as_bytes())
aad        = format!("{business_day}|{event_type}|{aggregate_id}|{seed.id}")
ciphertext = aes_gcm.encrypt(dek, payload, aad)
event_row  = (id=uuid_v4, business_day, ts, type, aggregate_id, seed_id=seed.id, payload_enc=ciphertext, ...)
```

Note: `event.key_id` column is renamed to `seed_id` (semantic change; the day part is now implicit from `business_day`).

### Decryption read path

```
seed       = cache.get(row.seed_id)
            // not found → AppError::Crypto("seed expired") (bouncer rotated
            // it out between the original write and this read)
dek        = blake3(seed.bytes, row.business_day.as_bytes())
aad        = format!("{}|{}|{}|{}", row.business_day, row.event_type, row.aggregate_id, row.seed_id)
plaintext  = aes_gcm.decrypt(dek, row.payload_enc, aad)
```

### Hole-tolerant warm-up

Each event row has `agg_seq INTEGER NOT NULL` — per-aggregate sequence number, assigned at write time as `MAX(agg_seq WHERE aggregate_id=?) + 1`. With UNIQUE(aggregate_id, agg_seq).

Warm-up algorithm:
1. Read all events for active business days, ordered by `(ts, id)`
2. For each aggregate (group by aggregate_id):
   - Collect all events
   - Verify the agg_seq sequence is unbroken from 1..N (no gaps)
   - Verify every event decrypts successfully
   - **If ANY event fails decrypt OR any agg_seq is missing → drop the entire aggregate**: do not apply any of its events; log warning with aggregate_id and reason
3. Apply surviving aggregates' events in `(ts, id)` order globally

Effect: a session whose first OrderPlaced was encrypted under a seed_id we no longer have is dropped entirely — no half-applied state. Reports for the day still capture what was successfully applied.

Holes happen when:
- Bouncer rotated out a seed that was used mid-session
- Pre-prod fiddling: someone deleted event rows manually

This is opportunistic recovery, not a guarantee — the operational invariant is "stay in normal mode end-to-end for a session."

### Print

Existing `print(kind, payload)` stub becomes:

```rust
pub fn print(kind: &str, payload: &serde_json::Value) {
    PRINT_QUEUE.enqueue(PrintJob { kind: kind.into(), payload: payload.clone(), target: None });
}
```

`PRINT_QUEUE` is a tokio mpsc + background task. Worker drains, POSTs to bouncer, retries with backoff on failure, persists pending jobs to a small SQLite-backed `print_queue` table so they survive cashier restart. Print is always a side effect — order placement never blocks on it.

### EOD

`eod/runner.rs` flow becomes:

1. Build report (unchanged)
2. `POST /reports/eod` to bouncer; if non-2xx or unreachable, mark eod_runs.status = `failed` with error, return; **do not delete event rows**, **do not mark `ok`**
3. On success: mark eod_runs.status = `ok`
4. `events.delete_day(business_day)` — same as today
5. Prune idempotency_keys + token_denylist — same as today

Removed:
- Local `daily_report` row INSERT
- Local `reports/<day>.json` file write

The `daily_report` table and `reports/` directory **are dropped entirely** in this work. Cashier holds no historical reports on disk.

---

## What gets deleted from the cashier

This is a substantial subtraction:

- `master.dek` table (no more wrapped DEKs on disk)
- `master.daily_report` table (no more local report rows)
- `reports/` directory (no more plaintext reports on disk)
- `apps/cashier/src-tauri/src/services/key_manager.rs` — replaced by `seed_cache.rs`
- `apps/cashier/src-tauri/src/services/utc_day.rs` — only `utc_day_of`-style helpers remain if needed elsewhere; rotation-day math goes
- `apps/cashier/src-tauri/src/rotation/` — entire module removed (no in-process rotation)
- `apps/cashier/src-tauri/src/crypto.rs` `Kek` type — DEKs are derived deterministically; no more wrapping
- `apps/cashier/src-tauri/src/keychain.rs` — KEK no longer in OS keychain (cashier holds no long-lived secrets)
- `apps/cashier/src-tauri/src/bootstrap.rs` — KEK bootstrap goes; auth signing key bootstrap stays
- HTTP `GET /admin/keys` endpoint and `Action::ViewKeys` — bouncer owns this view; cashier no longer sees seed material to list
- `cashier eod-now` CLI subcommand — keep, but the runner internally calls bouncer

What stays:
- Auth signing key (HMAC for bearer tokens) — independent concern, lives in keychain still
- `ts`-based event ordering, UUID-event-id was discussed but is **not adopted here** — without noise injection, autoincrement IDs are fine. (If forensic ID-leak becomes a concern later, swap independently.)

---

## Mock bouncer (`apps/bouncer-mock`)

Minimum viable mock for development and tests:

```
apps/bouncer-mock/
  Cargo.toml
  src/main.rs                  # axum on 127.0.0.1:7879
  src/seeds.rs                 # in-memory seed list; one default; bytes from a fixed test source
  src/printers.rs              # static stub list; POST /print writes to stdout + ./tmp/prints.log
  src/reports.rs               # POST /reports/eod writes JSON to ./tmp/reports/<day>.json
```

Behavior:
- Starts with a single seed, label `"dev-default"`, default=true. Seed bytes derived deterministically from a hardcoded constant so tests are reproducible.
- Optional `--seeds-file` CLI arg to load custom seed list from JSON for advanced tests.
- Writes nothing to OS keychain; doesn't simulate auth.

The mock is **NOT** what the production team builds. It exists so the cashier can be developed and tested end-to-end without depending on the real bouncer.

Cargo workspace: add `apps/bouncer-mock` to root members.

Dev workflow: a new pnpm script (or Makefile target) starts both processes:
```
pnpm dev  # starts cashier (Tauri) + bouncer-mock together
```

For Tauri integration: bouncer-mock can be bundled as a Tauri sidecar binary in dev builds; production deployments will have the real bouncer running independently.

---

## Schema impact

`apps/cashier/src-tauri/src/store/migrations/master/0007_drop_local_keys_and_reports.sql`:

```sql
DROP TABLE IF EXISTS dek;
DROP TABLE IF EXISTS daily_report;
```

`apps/cashier/src-tauri/src/store/migrations/events/0004_seed_id.sql`:

```sql
-- Rename event.key_id → event.seed_id (pre-prod, drop and recreate is acceptable).
DROP TABLE IF EXISTS event;
CREATE TABLE event (
  id                  INTEGER PRIMARY KEY AUTOINCREMENT,
  business_day        TEXT NOT NULL,
  ts                  INTEGER NOT NULL,
  type                TEXT NOT NULL,
  aggregate_id        TEXT NOT NULL,
  actor_staff         INTEGER,
  actor_name          TEXT,
  override_staff_id   INTEGER,
  override_staff_name TEXT,
  payload_enc         BLOB NOT NULL,
  seed_id             TEXT NOT NULL
);
CREATE INDEX idx_event_day      ON event(business_day);
CREATE INDEX idx_event_agg      ON event(aggregate_id, id);
CREATE INDEX idx_event_day_type ON event(business_day, type);
```

(Reverting to autoincrement id since UUID was driven by the noise plan that's no longer happening.)

---

## Threat model under the new arrangement

- **Cold cashier disk theft:** events ciphertext on disk; no key material on disk; no plaintext reports. Attacker recovers nothing without reaching bouncer too.
- **Warm cashier (logged-in box):** cashier process holds seed bytes in RAM. RAM dump → decryption of all events whose seeds are in cache. Same window as today within the bouncer's seed-retention policy.
- **Bouncer compromise:** depends on bouncer's own design (out of this spec). Worst case: attacker reads all seeds → can decrypt any event whose ciphertext they also have.
- **Network interception (loopback):** the seed bytes flow over `127.0.0.1`. Localhost traffic is observable to any process on the box but not over the network.
- **Crypto-shred:** bouncer removes a seed → cashier on next restart no longer caches it → events tagged with that seed_id become permanently undecryptable.
- **Reports off-box:** the plaintext-report leak is gone. Bouncer's report storage is its own threat model.
- **Coercion:** no defense (acknowledged previously).

---

## Known follow-ups (acknowledged, not in this spec)

- **`event.aggregate_id` is plaintext on disk.** Counts of distinct values per business_day reveal session/order counts to a forensic attacker even though payloads are encrypted. Mitigations require either keyed-hashing the aggregate_id (breaks cross-day session continuity unless careful) or noise rows. Track in a separate spec.

## Out of scope

- Bouncer's own threat model, storage, auth, deployment — owned by the bouncer team.
- Real printer drivers — bouncer concern.
- Report destinations / cloud sync / NAS / etc. — bouncer concern.
- WebAuthn, manager-PIN-derived KEK, SQLCipher, hardware tokens — all became moot once the cashier holds no long-lived secrets.
- Noise injection / UUID event IDs — superseded.
- Backward-compat migration from old `dek`/`daily_report` data — pre-prod, acceptable to drop.
