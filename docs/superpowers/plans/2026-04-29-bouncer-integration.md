# Bouncer Integration — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move key custody, printer routing, and report delivery out of the cashier into a separate localhost HTTP service called `bouncer`. Build a mock bouncer in this repo so cashier development remains end-to-end testable. Drop the local key/report storage that the bouncer replaces.

**Spec:** `docs/superpowers/specs/2026-04-29-bouncer-sidecar-design.md` — wire protocol and threat model.

**Big subtractions** (this plan removes a lot of code):
- `master.dek` table + key_manager + rotation scheduler + `/admin/keys` + `Action::ViewKeys`
- `master.daily_report` table + `reports/` directory writes
- KEK in OS keychain + crypto.rs `Kek` type + bootstrap KEK code
- Existing `print()` stub goes through queue+HTTP

---

## File Structure

```
apps/bouncer-mock/                                    # NEW workspace member
  Cargo.toml
  src/main.rs                                         # axum on 127.0.0.1:7879
  src/seeds.rs                                        # in-memory seeds + GET /seeds
  src/printers.rs                                     # GET /printers + POST /print → log file
  src/reports.rs                                      # POST /reports/eod → ./tmp/reports/<day>.json
  src/health.rs                                       # GET /health

apps/cashier/src-tauri/src/
  bouncer/
    mod.rs                                            # NEW
    client.rs                                         # NEW — reqwest blocking + retry helpers
    seed_cache.rs                                     # NEW — startup fetch; in-RAM map by id; default()
    print_queue.rs                                    # NEW — tokio mpsc + retry loop + persistence
  services/
    key_manager.rs                                    # REWRITTEN — derive DEK = blake3(seed, day)
                                                      #            seeds from seed_cache (no master.dek)
  services/utc_day.rs                                 # MODIFIED — strip rotation-only helpers
  rotation/                                           # DELETED
  crypto.rs                                           # MODIFIED — drop Kek; keep Dek/encrypt/decrypt
  keychain.rs                                         # MODIFIED — keep auth signing key path; remove KEK
  bootstrap.rs                                        # MODIFIED — auth signing key only; KEK gone
  print.rs                                            # REWRITTEN — enqueue to print_queue
  eod/runner.rs                                       # MODIFIED — POST /reports/eod replaces local writes
  http/routes/admin.rs                                # MODIFIED — drop /admin/keys + DekInfo route
  http/routes/reports.rs                              # MODIFIED — local report read still works for
                                                      #            CURRENT-day pre-EOD; otherwise 410 Gone
                                                      #            (or just drop the route entirely — see Task 6)
  acl/action.rs                                       # MODIFIED — drop ViewKeys
  acl/policy.rs                                       # MODIFIED — drop ViewKeys arm
  app_state.rs                                        # MODIFIED — drop kek; add bouncer client + seed cache
  lib.rs                                              # MODIFIED — bootstrap calls bouncer health + seeds;
                                                      #            removes rotation::spawn
  cli.rs                                              # MODIFIED — eod-now uses bouncer client
  store/migrations/master/
    0007_drop_local_keys_and_reports.sql              # NEW — drop dek + daily_report
  store/migrations/events/
    0004_seed_id.sql                                  # NEW — drop+recreate event with seed_id column

packages/shared/src/schemas/
  bouncer.ts                                          # NEW — zod for the bouncer wire types
  admin.ts                                            # MODIFIED — drop DekInfo

apps/admin/src/routes/
  reports.tsx                                         # MODIFIED — surfaces "off-box, see bouncer" if reports
                                                      # were the source; or drop the route
  authenticators.tsx / keys.tsx                       # n/a — was never built

Cargo.toml (workspace root)                            # MODIFIED — add apps/bouncer-mock
package.json (root)                                    # MODIFIED — `pnpm dev` script starts both
```

Decisions:
- **`event.seed_id` replaces `event.key_id`.** Since this is a destructive recreate (pre-prod), no backfill.
- **DEK derivation: `blake3(seed_bytes, business_day_str.as_bytes())`.** 32-byte output. `blake3` crate. Fast, no IV/salt needed (deterministic by design — same seed + same day = same DEK).
- **Print queue persists to a small SQLite table** in master.db: `print_queue (id INTEGER PK, kind TEXT, payload_json TEXT, target TEXT NULL, attempts INTEGER, last_error TEXT NULL, enqueued_at INTEGER)`. Worker drains FIFO, deletes on success.
- **Bootstrap is hard-fail.** If `GET /health` or `GET /seeds` fails at startup, cashier prints a clear message and exits non-zero. No cached-fallback behavior.
- **Mock bouncer is a separate Cargo binary** in `apps/bouncer-mock`, NOT a Tauri sidecar (yet). Run with `cargo run -p bouncer-mock` in dev. Production deployment: real bouncer runs separately.
- **EOD report POST is synchronous** — EOD does not mark `ok` until the POST succeeds. On failure, `eod_runs.status = 'failed'`, `error` populated, retried by next scheduler firing or manual `cashier eod-now`.
- **Reports are no longer queryable from cashier**. The current admin Reports route surfaces `daily_report` rows; after this work the table is gone, so the route either gets repointed to bouncer (NOT in scope) or returns "off-box" empty state. We'll make it return an explicit "reports are stored by bouncer; check bouncer for retrieval" empty state.

---

## Task 1: Bouncer mock workspace

**Files:**
- Create: `apps/bouncer-mock/{Cargo.toml,src/main.rs,src/seeds.rs,src/printers.rs,src/reports.rs,src/health.rs}`
- Modify: root `Cargo.toml` workspace members

- [ ] **Step 1: Cargo.toml**

```toml
[package]
name = "bouncer-mock"
version = "0.0.1"
edition = "2021"

[dependencies]
axum = "0.7"
tokio = { version = "1", features = ["macros", "rt-multi-thread", "signal"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
hex = "0.4"
blake3 = "1"
tracing = "0.1"
tracing-subscriber = "0.3"
tokio-util = "0.7"
```

Workspace root `Cargo.toml` add `"apps/bouncer-mock"` to `members`.

- [ ] **Step 2: main.rs binds 127.0.0.1:7879**

```rust
#[tokio::main]
async fn main() {
    tracing_subscriber::fmt::init();
    let app = axum::Router::new()
        .route("/health", axum::routing::get(health::get))
        .route("/seeds", axum::routing::get(seeds::list))
        .route("/printers", axum::routing::get(printers::list))
        .route("/print", axum::routing::post(printers::print))
        .route("/reports/eod", axum::routing::post(reports::eod));
    let addr = std::env::var("BOUNCER_BIND").unwrap_or_else(|_| "127.0.0.1:7879".into());
    let listener = tokio::net::TcpListener::bind(&addr).await.unwrap();
    tracing::info!("bouncer-mock listening on {addr}");
    axum::serve(listener, app).await.unwrap();
}
```

- [ ] **Step 3: seeds.rs**

```rust
pub async fn list() -> Json<Vec<Seed>> {
    Json(vec![Seed {
        id: "dev-default".into(),
        label: "Development default".into(),
        default: true,
        seed_hex: hex::encode(blake3::hash(b"lofi-pos-dev-seed-2026").as_bytes()),
    }])
}

#[derive(Serialize)]
pub struct Seed { pub id: String, pub label: String, pub default: bool, pub seed_hex: String }
```

- [ ] **Step 4: printers.rs + reports.rs**

`printers::print` writes incoming JSON line to `./tmp/prints.log` and returns `202 {"queued": true}`. `reports::eod` writes the report payload to `./tmp/reports/<business_day>.json` and returns `200 {"stored": true}`. `mkdir -p ./tmp` on first call.

- [ ] **Step 5: cargo build -p bouncer-mock + smoke test**

```bash
cargo run -p bouncer-mock &
curl -s http://127.0.0.1:7879/health           # → {"ok":true}
curl -s http://127.0.0.1:7879/seeds | jq .     # → [{...}]
kill %1
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(bouncer-mock): localhost HTTP stub for seeds/printers/reports"
```

---

## Task 2: Cashier bouncer client

**Files:**
- Create: `apps/cashier/src-tauri/src/bouncer/{mod.rs,client.rs}`
- Modify: `apps/cashier/src-tauri/Cargo.toml` (add `reqwest = { version = "0.12", features = ["json", "blocking"] }` and `blake3 = "1"`)
- Create: `packages/shared/src/schemas/bouncer.ts`

- [ ] **Step 1: client.rs**

```rust
use crate::error::{AppError, AppResult};
use serde::Deserialize;

pub struct BouncerClient {
    base: String,
    http: reqwest::blocking::Client,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SeedRow {
    pub id: String,
    pub label: String,
    pub default: bool,
    pub seed_hex: String,
}

impl BouncerClient {
    pub fn new(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            http: reqwest::blocking::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap(),
        }
    }

    pub fn health(&self) -> AppResult<()> {
        let r = self.http.get(format!("{}/health", self.base)).send()
            .map_err(|e| AppError::Internal(format!("bouncer health: {e}")))?;
        if !r.status().is_success() { return Err(AppError::Internal(format!("bouncer health {}", r.status()))); }
        Ok(())
    }

    pub fn list_seeds(&self) -> AppResult<Vec<SeedRow>> {
        let r = self.http.get(format!("{}/seeds", self.base)).send()
            .map_err(|e| AppError::Internal(format!("bouncer seeds: {e}")))?;
        if !r.status().is_success() { return Err(AppError::Internal(format!("bouncer seeds {}", r.status()))); }
        Ok(r.json().map_err(|e| AppError::Internal(format!("bouncer seeds parse: {e}")))?)
    }

    pub fn print(&self, kind: &str, payload: &serde_json::Value, target: Option<&str>) -> AppResult<()> {
        let body = serde_json::json!({ "kind": kind, "payload": payload, "target_printer_id": target });
        let r = self.http.post(format!("{}/print", self.base)).json(&body).send()
            .map_err(|e| AppError::Internal(format!("bouncer print: {e}")))?;
        if !r.status().is_success() { return Err(AppError::Internal(format!("bouncer print {}", r.status()))); }
        Ok(())
    }

    pub fn post_report(&self, business_day: &str, generated_at: i64, report: &serde_json::Value) -> AppResult<()> {
        let body = serde_json::json!({ "business_day": business_day, "generated_at": generated_at, "report": report });
        let r = self.http.post(format!("{}/reports/eod", self.base)).json(&body).send()
            .map_err(|e| AppError::Internal(format!("bouncer report: {e}")))?;
        if !r.status().is_success() { return Err(AppError::Internal(format!("bouncer report {}", r.status()))); }
        Ok(())
    }
}
```

- [ ] **Step 2: tests against running mock**

```rust
// integration test that spawns bouncer-mock as a subprocess; smoke endpoints
// alternative: test that hits a hand-rolled axum server in the test process
```

Recommend the latter — wire-format-only tests, no subprocess.

- [ ] **Step 3: shared zod schemas**

```ts
// packages/shared/src/schemas/bouncer.ts
import { z } from "zod";
export const BouncerSeed = z.object({
  id: z.string(),
  label: z.string(),
  default: z.boolean(),
  seed_hex: z.string().regex(/^[0-9a-f]{64}$/),
}).strict();
```

- [ ] **Step 4: cargo test bouncer::client**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(bouncer): cashier-side HTTP client + zod schemas"
```

---

## Task 3: Seed cache + KeyManager rewrite

**Files:**
- Create: `apps/cashier/src-tauri/src/bouncer/seed_cache.rs`
- Rewrite: `apps/cashier/src-tauri/src/services/key_manager.rs`
- Modify: `apps/cashier/src-tauri/src/services/utc_day.rs` (drop rotation helpers; keep what's still used)
- Delete: `apps/cashier/src-tauri/src/rotation/` (entire directory)
- Modify: `apps/cashier/src-tauri/src/crypto.rs` (drop `Kek`; keep `Dek` and AES-GCM helpers)
- Modify: `apps/cashier/src-tauri/src/keychain.rs` (drop KEK store/load; auth signing key stays)
- Modify: `apps/cashier/src-tauri/src/bootstrap.rs` (KEK init goes; auth signing key stays)
- Modify: `apps/cashier/src-tauri/src/app_state.rs` (drop `kek`; add `seed_cache: Arc<SeedCache>` and `bouncer: Arc<BouncerClient>`)

- [ ] **Step 1: seed_cache.rs**

```rust
use crate::bouncer::client::{BouncerClient, SeedRow};
use crate::error::{AppError, AppResult};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SeedCache {
    by_id: HashMap<String, [u8; 32]>,
    default_id: String,
}

impl SeedCache {
    pub fn fetch(client: &BouncerClient) -> AppResult<Self> {
        let rows = client.list_seeds()?;
        if rows.is_empty() {
            return Err(AppError::Internal("bouncer returned zero seeds".into()));
        }
        let mut by_id = HashMap::new();
        let mut default_id: Option<String> = None;
        for r in rows {
            let bytes = hex::decode(&r.seed_hex).map_err(|e| AppError::Internal(format!("seed hex: {e}")))?;
            if bytes.len() != 32 {
                return Err(AppError::Internal(format!("seed {} wrong length: {}", r.id, bytes.len())));
            }
            let mut arr = [0u8; 32];
            arr.copy_from_slice(&bytes);
            if r.default {
                if default_id.is_some() {
                    return Err(AppError::Internal("multiple default seeds".into()));
                }
                default_id = Some(r.id.clone());
            }
            by_id.insert(r.id, arr);
        }
        let default_id = default_id.ok_or_else(|| AppError::Internal("no default seed".into()))?;
        Ok(Self { by_id, default_id })
    }

    pub fn default_id(&self) -> &str { &self.default_id }
    pub fn default_seed(&self) -> &[u8; 32] { &self.by_id[&self.default_id] }
    pub fn get(&self, id: &str) -> AppResult<&[u8; 32]> {
        self.by_id.get(id).ok_or_else(|| AppError::Crypto(format!("seed expired for id {id}")))
    }
}
```

- [ ] **Step 2: KeyManager rewrite**

```rust
// services/key_manager.rs
use crate::bouncer::seed_cache::SeedCache;
use crate::crypto::Dek;
use crate::error::AppResult;
use std::sync::Arc;

pub struct KeyManager {
    cache: Arc<SeedCache>,
}

impl KeyManager {
    pub fn new(cache: Arc<SeedCache>) -> Self { Self { cache } }

    /// DEK for encrypting today's events. Returns (dek, seed_id).
    pub fn current_dek(&self, business_day: &str) -> (Dek, String) {
        let seed = self.cache.default_seed();
        let dek = derive_dek(seed, business_day);
        (dek, self.cache.default_id().to_string())
    }

    /// DEK for decrypting an existing event tagged with `seed_id`.
    pub fn dek_for(&self, seed_id: &str, business_day: &str) -> AppResult<Dek> {
        let seed = self.cache.get(seed_id)?;
        Ok(derive_dek(seed, business_day))
    }
}

fn derive_dek(seed: &[u8; 32], business_day: &str) -> Dek {
    let mut hasher = blake3::Hasher::new_keyed(seed);
    hasher.update(business_day.as_bytes());
    let out = hasher.finalize();
    Dek::from_bytes(out.as_bytes()).expect("blake3 output is 32 bytes")
}
```

(`Dek::from_bytes` already exists in `crypto.rs`; confirm during impl. If it doesn't take `&[u8; 32]`, adapt.)

Tests:
```rust
#[test] fn current_dek_matches_dek_for_with_same_inputs() { /* roundtrip */ }
#[test] fn dek_for_unknown_seed_returns_key_expired() { /* err is Crypto("seed expired ...") */ }
#[test] fn different_days_yield_different_deks() { /* … */ }
#[test] fn different_seeds_yield_different_deks() { /* … */ }
```

- [ ] **Step 3: delete rotation/**

```bash
git rm -rf apps/cashier/src-tauri/src/rotation
```

Remove `mod rotation;` from `lib.rs` and the `rotation::spawn(state.clone());` call.

- [ ] **Step 4: drop Kek from crypto.rs and keychain**

Keep `Dek`, `encrypt`, `decrypt`, `KEY_LEN`, `NONCE_LEN`, `TAG_LEN`. Delete `Kek` impl + tests.

In `keychain.rs`, keep `auth_signing_key` get/put; delete `kek` get/put (search for both names).

In `bootstrap.rs`, keep auth signing bootstrap; delete the KEK bootstrap (probably a `get_or_create_kek` function call).

- [ ] **Step 5: app_state.rs + EventService rewiring**

`AppState` no longer holds `kek`. Add `seed_cache: Arc<SeedCache>`, `bouncer: Arc<BouncerClient>`. KeyManager construction takes `Arc::clone(&seed_cache)`.

EventService.write needs to pass `business_day` into `KeyManager::current_dek(business_day)`. AAD changes:

```rust
let (dek, seed_id) = self.key_manager.current_dek(&business_day);
let aad = format!("{business_day}|{}|{}|{seed_id}", ev.event_type().as_str(), ctx.aggregate_id);
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
    seed_id: &seed_id,            // <-- renamed from key_id
})
```

EventService.read_decrypted:
```rust
let dek = self.key_manager.dek_for(&row.seed_id, &row.business_day)?;
let aad = format!("{}|{}|{}|{}", row.business_day, row.event_type, row.aggregate_id, row.seed_id);
let pt = dek.decrypt(&row.payload_enc, aad.as_bytes())?;
```

- [ ] **Step 6: cargo test services::key_manager + event_service**

After: cargo test compiles (other call sites in next task).

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(crypto): derive DEK from bouncer seeds; drop KEK + dek table + rotation"
```

---

## Task 4: Schema migrations + EventStore renames

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0007_drop_local_keys_and_reports.sql`
- Create: `apps/cashier/src-tauri/src/store/migrations/events/0004_seed_id.sql`
- Modify: `apps/cashier/src-tauri/src/store/events.rs` (`AppendEvent.key_id` → `seed_id`; `EventRow.key_id` → `seed_id`; SQL column name)
- Modify: `apps/cashier/src-tauri/src/store/master.rs` (drop dek + daily_report fns)

- [ ] **Step 1: master migration**

```sql
-- 0007_drop_local_keys_and_reports.sql
DROP TABLE IF EXISTS dek;
DROP TABLE IF EXISTS daily_report;
```

- [ ] **Step 2: events migration**

```sql
-- 0004_seed_id.sql
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

- [ ] **Step 3: events.rs field rename**

Search for `key_id` in `events.rs`; rename to `seed_id` in struct fields, SQL, and bindings.

- [ ] **Step 4: master.rs cleanup**

Delete `get_dek/put_dek/delete_dek/list_dek_days/delete_deks_older_than/DekInfo`. Delete any `daily_report`-related functions if they exist.

- [ ] **Step 5: cargo test --lib**

Compile + run unit tests.

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "refactor(store): drop dek+daily_report tables; rename key_id→seed_id"
```

---

## Task 5: Bootstrap rewiring (lib.rs + cli.rs)

**Files:**
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: `apps/cashier/src-tauri/src/cli.rs`

- [ ] **Step 1: lib.rs startup sequence**

Pseudocode:
```rust
let bouncer_url = std::env::var("LOFI_BOUNCER_URL").unwrap_or_else(|_| "http://127.0.0.1:7879".into());
let bouncer = Arc::new(BouncerClient::new(bouncer_url));
bouncer.health().expect("bouncer not reachable; start bouncer service first");
let seed_cache = Arc::new(SeedCache::fetch(&bouncer).expect("could not fetch seeds from bouncer"));
let key_manager = Arc::new(KeyManager::new(Arc::clone(&seed_cache)));

// (other AppState construction follows; rotation::spawn removed)
```

The `expect`s become formatted `eprintln!` + `std::process::exit(2)` for clean operator messages.

- [ ] **Step 2: cli.rs (eod-now subcommand)**

Same bouncer init flow; same hard-fail. Then runs `eod::runner::run_eod` which now calls bouncer.post_report.

- [ ] **Step 3: integration test for hard-fail**

```rust
#[test]
fn cashier_init_aborts_when_bouncer_unreachable() {
    // attempt to construct AppState with an unreachable bouncer URL;
    // expect the expected error path
}
```

- [ ] **Step 4: cargo test**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(cashier): bouncer health + seed fetch at startup; hard-fail if down"
```

---

## Task 6: EOD runner — POST report instead of local writes

**Files:**
- Modify: `apps/cashier/src-tauri/src/eod/runner.rs`
- Modify: `apps/cashier/src-tauri/src/eod/builder.rs` (output shape unchanged, just verify it serializes)

- [ ] **Step 1: runner change**

Replace the `daily_report` upsert + `reports/<day>.json` write with:

```rust
let report = build_report(state, business_day).await?;
let payload = serde_json::to_value(&report).unwrap();
state.bouncer.post_report(business_day, state.clock.now_ms(), &payload)?;
```

If the call returns `Err`, mark `eod_runs.status = 'failed'` with the error and return — do NOT delete event rows, do NOT mark ok.

On success: continue with idempotency/denylist prune + eod_runs ok update + events.delete_day (per existing flow).

- [ ] **Step 2: drop AppState.reports_dir**

No more local report directory.

- [ ] **Step 3: tests**

Update `run_marks_eod_runs_ok_writes_report_deletes_event_rows`:
- Use a stub bouncer (axum router in test) to receive POST /reports/eod and assert payload correctness
- Assert no local `daily_report` row anywhere (table is dropped)
- Assert no `reports/` dir written

- [ ] **Step 4: cargo test eod**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "refactor(eod): POST report to bouncer; drop local daily_report+reports/"
```

---

## Task 7: Print queue

**Files:**
- Create: `apps/cashier/src-tauri/src/bouncer/print_queue.rs`
- Modify: `apps/cashier/src-tauri/src/print.rs` (rewrite)
- Modify: `apps/cashier/src-tauri/src/store/master.rs` (add `print_queue` table)
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0008_print_queue.sql`
- Modify: `apps/cashier/src-tauri/src/lib.rs` (spawn worker)

- [ ] **Step 1: migration**

```sql
-- 0008_print_queue.sql
CREATE TABLE print_queue (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  kind         TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  target       TEXT,
  attempts     INTEGER NOT NULL DEFAULT 0,
  last_error   TEXT,
  enqueued_at  INTEGER NOT NULL,
  next_try_at  INTEGER NOT NULL
);
CREATE INDEX idx_print_queue_next ON print_queue(next_try_at);
```

- [ ] **Step 2: print.rs becomes enqueue**

```rust
// print.rs
use crate::store::master::Master;
use std::sync::{Arc, Mutex};

pub fn enqueue(master: &Mutex<Master>, kind: &str, payload: &serde_json::Value) {
    if let Err(e) = master.lock().unwrap().enqueue_print(kind, &payload.to_string(), None) {
        tracing::error!("enqueue print failed: {e}");
    }
}
```

- [ ] **Step 3: print_queue.rs background worker**

```rust
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let next = {
                let m = state.master.lock().unwrap();
                m.next_print_job(state.clock.now_ms())
            };
            match next {
                Ok(Some(job)) => {
                    let payload: serde_json::Value = serde_json::from_str(&job.payload_json).unwrap_or(serde_json::Value::Null);
                    match state.bouncer.print(&job.kind, &payload, job.target.as_deref()) {
                        Ok(()) => {
                            let _ = state.master.lock().unwrap().delete_print_job(job.id);
                        }
                        Err(e) => {
                            let backoff_ms = backoff_for(job.attempts);
                            let _ = state.master.lock().unwrap().reschedule_print_job(job.id, &e.to_string(), state.clock.now_ms() + backoff_ms);
                        }
                    }
                }
                Ok(None) => tokio::time::sleep(Duration::from_secs(2)).await,
                Err(e) => {
                    tracing::error!("print queue read failed: {e}");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
            }
        }
    });
}

fn backoff_for(attempts: i64) -> i64 {
    match attempts {
        0 => 1_000,
        1 => 5_000,
        2 => 15_000,
        3 => 60_000,
        _ => 300_000,
    }
}
```

- [ ] **Step 4: master.rs print_queue helpers**

`enqueue_print`, `next_print_job`, `delete_print_job`, `reschedule_print_job`.

- [ ] **Step 5: command_service side-effects use enqueue**

The existing print sites (OrderPlaced, PaymentTaken, SessionClosed) call `crate::print::enqueue(...)` instead of `crate::print::print(...)`. CommandService gets `Arc<Mutex<Master>>` already; pass it in.

- [ ] **Step 6: Spawn worker in lib.rs**

Alongside the EOD scheduler, spawn `bouncer::print_queue::spawn(state_arc.clone());`.

- [ ] **Step 7: tests**

```rust
#[tokio::test] async fn print_queue_drains_on_success() { /* ... */ }
#[tokio::test] async fn print_queue_retries_with_backoff_on_failure() { /* ... */ }
```

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(print): background queue + retries; OrderPlaced/PaymentTaken/SessionClosed enqueue"
```

---

## Task 8: Remove /admin/keys + reports route adjustment

**Files:**
- Modify: `apps/cashier/src-tauri/src/http/routes/admin.rs` (drop /admin/keys handler)
- Modify: `apps/cashier/src-tauri/src/http/routes/reports.rs` (adjust or drop)
- Modify: `apps/cashier/src-tauri/src/acl/{action,policy}.rs` (drop ViewKeys)
- Modify: `packages/shared/src/schemas/admin.ts` (drop DekInfo + DekInfoList)
- Modify: `apps/admin/src/routes/{reports,keys}.tsx` (cleanup)

- [ ] **Step 1: admin.rs**

Delete the list_keys handler and its route. Remove `Action::ViewKeys` references.

- [ ] **Step 2: reports.rs**

`GET /admin/reports` returns `200 []` (empty list — reports live off-box now). `GET /admin/reports/:day` returns `404` with body `{"code":"reports_off_box","message":"Reports are stored by the bouncer service. See bouncer for retrieval."}`. Or drop the routes entirely.

Recommend dropping the routes — their semantics are no longer meaningful.

- [ ] **Step 3: ACL cleanup**

`acl/action.rs`: remove `ViewKeys`. Also remove `ViewReports` if reports route is dropped.

`acl/policy.rs`: remove the corresponding match arms.

- [ ] **Step 4: shared schemas**

Remove `DekInfo`, `DekInfoList`, `DailyReport`-related types from admin.ts. Keep zod for `Settings`, spot/staff/product CRUD, etc.

- [ ] **Step 5: admin SPA**

`apps/admin/src/routes/reports.tsx` and any `keys.tsx` (was never built — verify): if a Reports nav link exists, replace with informational empty state ("Reports are stored by the bouncer service") or remove the link.

- [ ] **Step 6: Final cargo test + clippy + all typechecks**

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "refactor(admin): drop /admin/keys + local reports route (off-box now)"
```

---

## Task 9: Final gate

- [ ] `cargo test` — count likely DECREASES (rotation/dek/daily_report tests gone) but should still be ~190+
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] All workspace typechecks
- [ ] Manual smoke:
  ```bash
  # Terminal 1: bouncer
  cargo run -p bouncer-mock
  # Terminal 2: cashier
  pnpm --filter @lofi-pos/cashier tauri dev
  ```
  Verify cashier boots, login works, order placement works, EOD posts to bouncer (`./tmp/reports/<day>.json` appears), print actions hit `./tmp/prints.log`.

- [ ] Cashier startup with bouncer NOT running: verify clean error message and non-zero exit.

---

## Done

End state:
- `bouncer-mock` is a runnable Cargo binary serving the localhost wire protocol the real bouncer team will implement against.
- Cashier holds zero long-lived secrets on disk (no KEK, no wrapped DEKs).
- Cashier holds zero historical reports on disk (no `daily_report` table, no `reports/` dir).
- DEKs are derived deterministically: `blake3(seed, business_day)`. Seeds come from bouncer at startup, cached in RAM.
- EOD POSTs the day's report to bouncer; only marks `ok` after bouncer confirms.
- Prints flow through a persistent FIFO queue with backoff retry; orders never block on print.
- Bouncer crypto-shred = remove a seed → next cashier restart drops it from cache → events tagged with that seed_id are unrecoverable.

Remaining for follow-up:
- Real bouncer implementation (other team's spec).
- Tauri sidecar bundling of bouncer-mock for dev convenience.
- Production deployment story (systemd unit / Windows service for bouncer).
- Recovery story when bouncer is unreachable for an extended period (currently: cashier offline).
