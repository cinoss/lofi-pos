# Foundation Plan C — Auth, ACL, Validation, Tauri Commands

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Wire Plan B's primitives into a usable command layer the cashier UI can call. PIN-based auth, role-based ACL with override flow, writer-side validation against current state, idempotent commands, and a `tauri::command` surface for every domain operation. End state: a React caller can invoke `login(pin)`, `open_session(...)`, `place_order(...)`, etc. through Tauri IPC and get back projections; ACL violations and validation errors are surfaced as structured `AppError`s.

**Architecture:** Refactor `EventWriter` (and friends) into owned-`Arc` services so they live in `tauri::State` for the process lifetime. Introduce a `CommandService` facade that bundles ACL guard + idempotency + validation + write + projection load, so each `#[tauri::command]` is a thin shim. Auth = Argon2id PIN hash + HMAC-signed bearer token (signing key in OS keychain). ACL = single `policy::check(action, actor, ctx)` function returning `Allow | Deny | OverrideRequired(role)`.

**Tech Stack:** Adds `argon2`, `hmac`, `sha2`, `subtle`, `base64`, `uuid` to existing Rust deps. No new TS deps yet (UI scaffold consumes via `@tauri-apps/api`).

**Spec:** `docs/superpowers/specs/2026-04-27-foundation-design.md` — sections "Auth", "Authorization (ACL)", "API Surface" (only the Tauri-command parts; HTTP is Plan D).

**Builds on:** Plan A + B. Per the Plan C prep notes in memory, this addresses items 1–8 except HTTP/axum (deferred to Plan D).

**Out of scope (deferred):**
- axum HTTP+WS server (Plan D)
- packages/shared TS client (Plan D)
- EOD pipeline + print stub call sites (Plan E)

---

## File Structure

```
apps/cashier/src-tauri/src/
  error.rs                              # MODIFIED — Display already covers HTTP-style mapping; add ErrorKind helper
  lib.rs                                # MODIFIED — AppState now owns EventStore + services + auth
  app_state.rs                          # NEW — AppState extracted; Arc<...> wiring
  auth/
    mod.rs                              # NEW
    pin.rs                              # NEW — Argon2id hash + verify
    token.rs                            # NEW — HMAC-signed bearer tokens (sign/verify, signing key in keychain)
    service.rs                          # NEW — AuthService (login, verify_token, logout)
  acl/
    mod.rs                              # NEW
    role.rs                             # NEW — Role enum, total_ord (Owner > Manager > Cashier > Staff)
    action.rs                           # NEW — Action enum (one variant per protected operation)
    policy.rs                           # NEW — check(action, actor, ctx) -> Decision
  services/
    event_writer.rs                     # MODIFIED — refactor to Arc fields (renamed EventService)
    event_writer.rs → event_service.rs  # RENAME
    idempotency.rs                      # NEW — InsertOrConflict semantics on master.idempotency_key table
    validation.rs                       # NEW — pre-write checks (Order/Payment/Session invariants)
    command_service.rs                  # NEW — orchestrates: ACL → idempotency → validate → write → project
    staff.rs                            # NEW — Staff CRUD on master.db (needed for login lookup, ACL actor)
  commands/
    mod.rs                              # NEW
    auth_cmd.rs                         # NEW — login, logout, current_user
    session_cmd.rs                      # NEW — open, close, transfer, merge, split, list_active, get
    order_cmd.rs                        # NEW — place_order, cancel_item, return_item
    payment_cmd.rs                      # NEW — take_payment
    catalog_cmd.rs                      # NEW — list_rooms, list_tables, list_products (read-only, no events)
  store/
    master.rs                           # MODIFIED — add staff CRUD; add idempotency_key, auth_signing_key migration
    migrations/master/
      0002_staff_seed.sql               # NEW — owner role staff seed (PIN set on first run)
      0003_idempotency.sql              # NEW — idempotency_key table
tests/
  auth_integration.rs                   # NEW — PIN/login/token round-trips
  acl_matrix.rs                         # NEW — full permission matrix
  validation_integration.rs             # NEW — writer-side validation rejects invalid events
  commands_integration.rs               # NEW — 1-2 happy-path Tauri command flows (use the service layer directly; Tauri runtime not required)
```

Notable decisions baked in here:
- **Rename `EventWriter` → `EventService`** since it now owns its dependencies and serves both write and read paths.
- **`AppState` extracted to its own file** — was a 4-field stub in `lib.rs`; will grow to ~10 fields.
- **Auth signing key** is a *second* keychain entry (`<service>.auth-signing`), separate from the KEK. Same `KeyStore` abstraction. Initialized at first run alongside the KEK.
- **`SessionMerged` aggregate-id contract:** write event ONCE under the **target** session's aggregate_id; sources don't see the event in their own aggregate stream. Read-time, when the UI needs to display "session X is merged into Y," `CommandService` provides a helper that scans cross-aggregate (cheap because indexed). Same for `SessionSplit`.
- **Idempotency key:** required on every write command. Stored in a `master.idempotency_key` table with a UNIQUE constraint; second submission returns the cached projection from the first write.

---

## Task 1: Add new Rust deps

**Files:** Modify `apps/cashier/src-tauri/Cargo.toml`

- [ ] **Step 1: Add to `[dependencies]`**

```toml
argon2 = "0.5"
hmac = "0.12"
sha2 = "0.10"
subtle = "2.6"
base64 = "0.22"
uuid = { version = "1", features = ["v4"] }
```

- [ ] **Step 2: Verify**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: deps resolve, no compile errors.

- [ ] **Step 3: Commit**

```bash
git add apps/cashier/src-tauri/Cargo.toml apps/cashier/src-tauri/Cargo.lock
git commit -m "feat(cashier): add argon2, hmac, sha2, subtle, base64, uuid deps"
```

---

## Task 2: Extract `AppState` to its own file

**Files:**
- Create: `apps/cashier/src-tauri/src/app_state.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Move `AppState` from `lib.rs` to `app_state.rs`**

Create `apps/cashier/src-tauri/src/app_state.rs`:
```rust
use crate::crypto::Kek;
use crate::store::master::Master;
use std::sync::Mutex;

/// Tauri-managed shared state. Held by the runtime for process lifetime;
/// dropped on shutdown (KEK zeroizes via `ZeroizeOnDrop`).
///
/// Plan C will grow this with `events`, `clock`, `auth`, `commands` — for
/// now it carries Plan A's pair so the rename lands without behavior change.
pub struct AppState {
    pub kek: Kek,
    pub master: Mutex<Master>,
}
```

- [ ] **Step 2: Update `lib.rs`**

Replace the `AppState` definition in `lib.rs` with `pub mod app_state;` and `use app_state::AppState;` (still constructed in `run`, no behavior change).

- [ ] **Step 3: Verify**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all 67 tests pass.

Run: `cargo clippy --all-targets -- -D warnings && cargo fmt --check`
Expected: clean.

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/app_state.rs apps/cashier/src-tauri/src/lib.rs
git commit -m "refactor(cashier): extract AppState to its own module"
```

---

## Task 3: Rename `event_writer.rs` → `event_service.rs` and refactor to `Arc` fields

**Files:**
- Move: `services/event_writer.rs` → `services/event_service.rs`
- Modify: `services/mod.rs`
- Modify: any consumers (only the `tests/projection_integration.rs` import path)

- [ ] **Step 1: Rename module + struct**

```bash
git mv apps/cashier/src-tauri/src/services/event_writer.rs \
       apps/cashier/src-tauri/src/services/event_service.rs
```

Update `apps/cashier/src-tauri/src/services/mod.rs`:
```rust
pub mod day_key;
pub mod event_service;
```

In `services/event_service.rs`, rename the struct `EventWriter` → `EventService`. Remove the `TODO(plan-c)` comment (now resolved).

- [ ] **Step 2: Refactor fields from `&'a` to `Arc`**

Change the struct from:
```rust
pub struct EventService<'a> {
    pub master: &'a Master,
    pub events: &'a EventStore,
    pub kek: &'a Kek,
    pub clock: &'a dyn Clock,
    pub cutoff_hour: u32,
    pub tz: FixedOffset,
}
```
to:
```rust
use std::sync::{Arc, Mutex};

pub struct EventService {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub kek: Arc<Kek>,
    pub clock: Arc<dyn Clock>,
    pub cutoff_hour: u32,
    pub tz: FixedOffset,
}
```

(`Master` keeps its outer `Mutex` because `rusqlite::Connection` is `!Sync`. `EventStore` does NOT need a Mutex if its `&self` methods only do parameterized queries — verify by re-reading `events.rs`. If they all take `&self`, an `Arc<EventStore>` suffices. If any take `&mut self`, wrap in `Mutex`.)

- [ ] **Step 3: Update method bodies**

`write` and `read_decrypted` previously dereferenced `self.master`, `self.events`, etc. Update each:
- `self.master.get_day_key(...)` → `self.master.lock().unwrap().get_day_key(...)`
- `self.events.append(...)` → `self.events.append(...)` (no change if EventStore has `&self` methods)
- `self.kek.unwrap(...)` → `self.kek.unwrap(...)` (Arc derefs)
- `self.clock.now()` → `self.clock.now()` (Arc<dyn Clock> derefs)
- `day_key::get_or_create(self.master, self.kek, ...)` → `day_key::get_or_create(&self.master.lock().unwrap(), &self.kek, ...)`

`day_key::get_or_create` signature must be updated to take `&Master, &Kek` (already does). Just the call site needs the lock + deref.

- [ ] **Step 4: Update tests**

In `services/event_service.rs` `mod tests`, change the `rig()` helper and each test that constructs `EventService`:

```rust
fn rig() -> (Arc<Mutex<Master>>, Arc<EventStore>, Arc<Kek>, Arc<MockClock>) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    (master, events, kek, clock)
}
```

Each test then:
```rust
let (master, events, kek, clock) = rig();
let svc = EventService {
    master: master.clone(), events: events.clone(), kek: kek.clone(),
    clock: clock.clone(), cutoff_hour: 11,
    tz: FixedOffset::east_opt(7 * 3600).unwrap(),
};
```

Keep the `master`/`events`/`clock` references for direct assertions (e.g., `events.list_for_day(...)`).

In `tests/projection_integration.rs`, do the same conversion. The integration test `EventService { ... }` construction now uses Arc clones too. Use the same `rig()` style or inline the Arc setup.

- [ ] **Step 5: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 67 tests pass. Diagnose any borrow-checker failures (most common: `MutexGuard` not living long enough across encrypt + append; consider scoping the lock tightly).

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "refactor(cashier): EventService with Arc fields (Tauri-state ready)

EventService (renamed from EventWriter) now owns its dependencies
via Arc, suitable for tauri::State<EventService> which requires
'static. Master stays inside Mutex (rusqlite::Connection is !Sync);
EventStore is Arc-only since its methods take &self."
```

---

## Task 4: Wire EventService + Clock into `AppState` setup

**Files:**
- Modify: `apps/cashier/src-tauri/src/app_state.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Extend `AppState`**

```rust
use crate::crypto::Kek;
use crate::services::event_service::EventService;
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::Clock;
use std::sync::{Arc, Mutex};

pub struct AppState {
    pub kek: Arc<Kek>,
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub clock: Arc<dyn Clock>,
    pub event_service: EventService,
    // Plan C will add: pub auth: AuthService, pub commands: CommandService
}
```

- [ ] **Step 2: Update `lib.rs::run` setup**

```rust
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()
                .map_err(|e| crate::error::AppError::Config(format!("app_data_dir: {e}")))?;
            std::fs::create_dir_all(&data_dir)?;

            let ks = keychain::OsKeyStore::new(keychain::SERVICE);
            let kek = Arc::new(bootstrap::load_or_init_kek(&ks)?);

            let master_path = data_dir.join("master.db");
            let master = Arc::new(Mutex::new(store::master::Master::open(&master_path)?));
            tracing::info!(?master_path, "master db opened");

            let events_path = data_dir.join("events.db");
            let events = Arc::new(store::events::EventStore::open(&events_path)?);
            tracing::info!(?events_path, "events db opened");

            let clock: Arc<dyn time::Clock> = Arc::new(time::SystemClock);

            // Load TZ + cutoff from settings
            let (cutoff_hour, tz) = load_business_day_settings(&master.lock().unwrap())?;

            let event_service = services::event_service::EventService {
                master: master.clone(),
                events: events.clone(),
                kek: kek.clone(),
                clock: clock.clone(),
                cutoff_hour,
                tz,
            };

            app.manage(app_state::AppState {
                kek, master, events, clock, event_service,
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn load_business_day_settings(master: &store::master::Master)
    -> crate::error::AppResult<(u32, chrono::FixedOffset)>
{
    let cutoff = master.get_setting("business_day_cutoff_hour")?
        .ok_or_else(|| crate::error::AppError::Config("business_day_cutoff_hour missing".into()))?
        .parse::<u32>()
        .map_err(|e| crate::error::AppError::Config(format!("cutoff parse: {e}")))?;
    let offset = master.get_setting("business_day_tz_offset_seconds")?
        .ok_or_else(|| crate::error::AppError::Config("business_day_tz_offset_seconds missing".into()))?
        .parse::<i32>()
        .map_err(|e| crate::error::AppError::Config(format!("tz parse: {e}")))?;
    let tz = chrono::FixedOffset::east_opt(offset)
        .ok_or_else(|| crate::error::AppError::Config(format!("invalid tz offset: {offset}")))?;
    Ok((cutoff, tz))
}
```

- [ ] **Step 3: Verify**

Run: `cd apps/cashier/src-tauri && cargo check && cargo test`
Expected: compiles, all tests pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): wire EventStore + Clock + EventService into AppState"
```

---

## Task 5: Argon2id PIN hash + verify

**Files:**
- Create: `apps/cashier/src-tauri/src/auth/mod.rs`
- Create: `apps/cashier/src-tauri/src/auth/pin.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Create `auth/mod.rs`**

```rust
pub mod pin;
```

- [ ] **Step 2: Implement PIN hash + verify**

Create `apps/cashier/src-tauri/src/auth/pin.rs`:
```rust
use crate::error::{AppError, AppResult};
use argon2::{
    password_hash::{
        rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString,
    },
    Argon2,
};

/// Hash a PIN with Argon2id. Output is the encoded `$argon2id$...` string,
/// suitable for storage in `staff.pin_hash`.
pub fn hash_pin(pin: &str) -> AppResult<String> {
    let salt = SaltString::generate(&mut OsRng);
    let argon = Argon2::default();
    Ok(argon
        .hash_password(pin.as_bytes(), &salt)
        .map_err(|e| AppError::Crypto(format!("hash_pin: {e}")))?
        .to_string())
}

/// Verify a PIN against a stored Argon2 hash. Returns Ok(true) on match,
/// Ok(false) on mismatch, Err on malformed hash.
pub fn verify_pin(pin: &str, stored_hash: &str) -> AppResult<bool> {
    let parsed = PasswordHash::new(stored_hash)
        .map_err(|e| AppError::Crypto(format!("verify_pin parse: {e}")))?;
    Ok(Argon2::default()
        .verify_password(pin.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_roundtrip() {
        let h = hash_pin("1234").unwrap();
        assert!(verify_pin("1234", &h).unwrap());
    }

    #[test]
    fn wrong_pin_does_not_verify() {
        let h = hash_pin("1234").unwrap();
        assert!(!verify_pin("0000", &h).unwrap());
    }

    #[test]
    fn malformed_hash_errors() {
        assert!(verify_pin("1234", "not-a-hash").is_err());
    }

    #[test]
    fn two_hashes_of_same_pin_differ() {
        // Argon2 uses random salt; same plaintext → different ciphertext
        let a = hash_pin("1234").unwrap();
        let b = hash_pin("1234").unwrap();
        assert_ne!(a, b);
        assert!(verify_pin("1234", &a).unwrap());
        assert!(verify_pin("1234", &b).unwrap());
    }
}
```

- [ ] **Step 3: Wire into lib**

Add to `lib.rs`:
```rust
pub mod auth;
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib auth::pin`
Expected: 4 tests pass.

- [ ] **Step 5: Commit**

```bash
git add apps/cashier/src-tauri/src/auth/mod.rs apps/cashier/src-tauri/src/auth/pin.rs apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): Argon2id PIN hash + verify"
```

---

## Task 6: HMAC-signed bearer tokens

**Files:**
- Create: `apps/cashier/src-tauri/src/auth/token.rs`
- Modify: `apps/cashier/src-tauri/src/auth/mod.rs`
- Modify: `apps/cashier/src-tauri/src/bootstrap.rs` (add auth signing key load/init)

- [ ] **Step 1: Implement token sign + verify**

Create `apps/cashier/src-tauri/src/auth/token.rs`:
```rust
use crate::acl::role::Role;
use crate::error::{AppError, AppResult};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use subtle::ConstantTimeEq;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenClaims {
    pub staff_id: i64,
    pub role: Role,
    /// unix-ms expiry
    pub exp: i64,
}

/// Sign claims with the auth signing key. Output: base64url(claims_json).base64url(sig)
pub fn sign(claims: &TokenClaims, signing_key: &[u8]) -> AppResult<String> {
    let json = serde_json::to_vec(claims)
        .map_err(|e| AppError::Internal(format!("token serialize: {e}")))?;
    let body_b64 = URL_SAFE_NO_PAD.encode(&json);
    let mut mac = HmacSha256::new_from_slice(signing_key)
        .map_err(|e| AppError::Crypto(format!("hmac key: {e}")))?;
    mac.update(body_b64.as_bytes());
    let sig = mac.finalize().into_bytes();
    let sig_b64 = URL_SAFE_NO_PAD.encode(sig);
    Ok(format!("{body_b64}.{sig_b64}"))
}

/// Verify and parse a token. Checks HMAC (constant-time) and expiry.
pub fn verify(token: &str, signing_key: &[u8], now_ms: i64) -> AppResult<TokenClaims> {
    let (body_b64, sig_b64) = token
        .split_once('.')
        .ok_or(AppError::Unauthorized)?;

    let mut mac = HmacSha256::new_from_slice(signing_key)
        .map_err(|e| AppError::Crypto(format!("hmac key: {e}")))?;
    mac.update(body_b64.as_bytes());
    let expected = mac.finalize().into_bytes();
    let provided = URL_SAFE_NO_PAD
        .decode(sig_b64)
        .map_err(|_| AppError::Unauthorized)?;

    if !bool::from(provided.ct_eq(&expected[..])) {
        return Err(AppError::Unauthorized);
    }

    let body = URL_SAFE_NO_PAD
        .decode(body_b64)
        .map_err(|_| AppError::Unauthorized)?;
    let claims: TokenClaims =
        serde_json::from_slice(&body).map_err(|_| AppError::Unauthorized)?;

    if now_ms >= claims.exp {
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> Vec<u8> {
        b"0123456789abcdef0123456789abcdef".to_vec()
    }
    fn claims(exp: i64) -> TokenClaims {
        TokenClaims { staff_id: 7, role: Role::Cashier, exp }
    }

    #[test]
    fn sign_then_verify_roundtrip() {
        let k = key();
        let t = sign(&claims(2_000_000_000_000), &k).unwrap();
        let parsed = verify(&t, &k, 1_000).unwrap();
        assert_eq!(parsed.staff_id, 7);
        assert_eq!(parsed.role, Role::Cashier);
    }

    #[test]
    fn wrong_key_rejected() {
        let t = sign(&claims(2_000_000_000_000), &key()).unwrap();
        let other = b"ffffffffffffffffffffffffffffffff".to_vec();
        assert!(matches!(verify(&t, &other, 1_000), Err(AppError::Unauthorized)));
    }

    #[test]
    fn expired_token_rejected() {
        let k = key();
        let t = sign(&claims(100), &k).unwrap();
        assert!(matches!(verify(&t, &k, 200), Err(AppError::Unauthorized)));
    }

    #[test]
    fn tampered_body_rejected() {
        let k = key();
        let mut t = sign(&claims(2_000_000_000_000), &k).unwrap();
        // Flip one byte in the body half
        let bytes: &mut [u8] = unsafe { t.as_bytes_mut() };
        bytes[0] ^= 0x01;
        assert!(matches!(verify(&t, &k, 1_000), Err(AppError::Unauthorized)));
    }

    #[test]
    fn malformed_token_rejected() {
        assert!(matches!(verify("no-dot-here", &key(), 1_000), Err(AppError::Unauthorized)));
    }
}
```

- [ ] **Step 2: Update `auth/mod.rs`**

```rust
pub mod pin;
pub mod token;
```

- [ ] **Step 3: Add auth signing key to bootstrap**

Modify `apps/cashier/src-tauri/src/bootstrap.rs`:

After `KEK_NAME`, add:
```rust
pub(crate) const AUTH_SIGNING_NAME: &str = "auth-signing";
pub(crate) const AUTH_SIGNING_LEN: usize = 32;

/// Load existing auth signing key from keystore or generate + persist a fresh one.
/// Used by `auth::token::sign`/`verify`.
pub fn load_or_init_auth_signing(ks: &dyn KeyStore) -> AppResult<Vec<u8>> {
    if let Some(bytes) = ks.get(AUTH_SIGNING_NAME)? {
        if bytes.len() == AUTH_SIGNING_LEN {
            tracing::info!("auth signing key loaded from keystore");
            return Ok(bytes);
        }
        return Err(crate::error::AppError::Crypto(
            "stored auth signing key has wrong length".into(),
        ));
    }
    use rand::RngCore;
    let mut bytes = vec![0u8; AUTH_SIGNING_LEN];
    rand::thread_rng().fill_bytes(&mut bytes);
    ks.set(AUTH_SIGNING_NAME, &bytes)?;
    tracing::info!("auth signing key generated and stored (first run)");
    Ok(bytes)
}

#[cfg(test)]
mod auth_signing_tests {
    use super::*;
    use crate::keychain::test_support::MemKeyStore;

    #[test]
    fn first_run_generates_auth_signing() {
        let ks = MemKeyStore::default();
        let k = load_or_init_auth_signing(&ks).unwrap();
        assert_eq!(k.len(), AUTH_SIGNING_LEN);
        assert_eq!(ks.get(AUTH_SIGNING_NAME).unwrap().as_deref(), Some(&k[..]));
    }

    #[test]
    fn second_run_returns_same_auth_signing() {
        let ks = MemKeyStore::default();
        let a = load_or_init_auth_signing(&ks).unwrap();
        let b = load_or_init_auth_signing(&ks).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn wrong_length_auth_signing_returns_error() {
        let ks = MemKeyStore::default();
        ks.set(AUTH_SIGNING_NAME, &[0u8; 16]).unwrap();
        assert!(load_or_init_auth_signing(&ks).is_err());
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib auth::token bootstrap::auth_signing_tests`
Expected: 5 token tests + 3 auth-signing tests = 8 pass.

(Note: `Role` doesn't exist yet — Task 7 creates it. Token tests will fail to compile until Task 7. **Alternative ordering:** swap Tasks 6 and 7 OR define a temporary placeholder `Role` enum here. Recommended: do Task 7 first, then Task 6. The plan steps 5-step list within Task 6 still applies.)

**Adjusted execution order:** complete Task 7's `acl/role.rs` BEFORE Task 6's tests can compile. The implementing agent should detect this and reorder.

- [ ] **Step 5: Commit (after Task 7's `Role` exists)**

```bash
git add -u
git commit -m "feat(cashier): HMAC-signed bearer tokens + auth signing key bootstrap"
```

---

## Task 7: ACL types — `Role`, `Action`

**Files:**
- Create: `apps/cashier/src-tauri/src/acl/mod.rs`
- Create: `apps/cashier/src-tauri/src/acl/role.rs`
- Create: `apps/cashier/src-tauri/src/acl/action.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Create `Role`**

Create `apps/cashier/src-tauri/src/acl/role.rs`:
```rust
use serde::{Deserialize, Serialize};

/// Staff role hierarchy. `Owner > Manager > Cashier > Staff`.
/// Stored as the lowercase string in `staff.role` (CHECK constraint pins these).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Staff,
    Cashier,
    Manager,
    Owner,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::Staff   => "staff",
            Role::Cashier => "cashier",
            Role::Manager => "manager",
            Role::Owner   => "owner",
        }
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "staff"   => Some(Role::Staff),
            "cashier" => Some(Role::Cashier),
            "manager" => Some(Role::Manager),
            "owner"   => Some(Role::Owner),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ordering() {
        assert!(Role::Owner > Role::Manager);
        assert!(Role::Manager > Role::Cashier);
        assert!(Role::Cashier > Role::Staff);
    }

    #[test]
    fn parse_roundtrip() {
        for r in [Role::Staff, Role::Cashier, Role::Manager, Role::Owner] {
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("nope"), None);
    }

    #[test]
    fn serde_lowercase() {
        let s = serde_json::to_string(&Role::Manager).unwrap();
        assert_eq!(s, "\"manager\"");
        let r: Role = serde_json::from_str("\"owner\"").unwrap();
        assert_eq!(r, Role::Owner);
    }
}
```

- [ ] **Step 2: Create `Action`**

Create `apps/cashier/src-tauri/src/acl/action.rs`:
```rust
/// Every protected operation, named for the policy matrix.
/// One variant per Tauri command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    // session
    OpenSession,
    CloseSession,
    TransferSession,
    MergeSessions,
    SplitSession,
    // order
    PlaceOrder,
    CancelOrderItemSelf,    // own item, within grace window
    CancelOrderItemAny,     // anyone's, anytime
    ReturnOrderItem,
    // payment
    TakePayment,
    ApplyDiscountSmall,     // ≤ threshold
    ApplyDiscountLarge,     // > threshold
    // catalog (read)
    ListRooms,
    ListTables,
    ListProducts,
    // session (read)
    ListActiveSessions,
    GetSession,
    // reports
    ViewLiveReports,
    RunEod,
    // admin
    EditMenu,
    EditRecipes,
    EditStaff,
    EditSettings,
}
```

- [ ] **Step 3: Wire into lib + create mod.rs**

Create `apps/cashier/src-tauri/src/acl/mod.rs`:
```rust
pub mod action;
pub mod policy;
pub mod role;

pub use action::Action;
pub use role::Role;
```

(`policy` is Task 8 — declared here so the module structure is final.)

Add to `apps/cashier/src-tauri/src/lib.rs`:
```rust
pub mod acl;
```

- [ ] **Step 4: Stub `policy.rs` so module compiles**

Create `apps/cashier/src-tauri/src/acl/policy.rs`:
```rust
// Implemented in Task 8.
```

- [ ] **Step 5: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib acl::role`
Expected: 3 tests pass.

- [ ] **Step 6: Commit**

```bash
git add apps/cashier/src-tauri/src/acl apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): ACL Role + Action enums"
```

---

## Task 8: ACL `policy::check`

**Files:**
- Modify: `apps/cashier/src-tauri/src/acl/policy.rs`

- [ ] **Step 1: Implement policy**

Replace `apps/cashier/src-tauri/src/acl/policy.rs`:
```rust
use crate::acl::{Action, Role};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny,
    OverrideRequired(Role),
}

#[derive(Debug, Clone, Copy)]
pub struct PolicyCtx {
    pub discount_pct: Option<u32>,
    pub discount_threshold_pct: u32,
    pub within_cancel_grace: bool,
    pub is_self: bool,
}

impl Default for PolicyCtx {
    fn default() -> Self {
        Self { discount_pct: None, discount_threshold_pct: 10,
               within_cancel_grace: false, is_self: false }
    }
}

/// Decide whether `actor` may perform `action` under `ctx`.
/// Mirrors the spec's permission matrix.
pub fn check(action: Action, actor: Role, ctx: PolicyCtx) -> Decision {
    use Action::*;
    use Decision::*;

    let allow_at = |min: Role| -> Decision {
        if actor >= min { Allow } else { OverrideRequired(min) }
    };

    match action {
        // unrestricted
        OpenSession | PlaceOrder | ListRooms | ListTables
        | ListProducts | ListActiveSessions | GetSession => Allow,

        // self-cancel within grace window
        CancelOrderItemSelf => {
            if ctx.is_self && ctx.within_cancel_grace { Allow }
            else { OverrideRequired(Role::Manager) }
        }

        // cashier+
        CloseSession | TakePayment => allow_at(Role::Cashier),
        ApplyDiscountSmall => {
            // cashier may apply if within threshold
            match ctx.discount_pct {
                Some(p) if p <= ctx.discount_threshold_pct => allow_at(Role::Cashier),
                _ => OverrideRequired(Role::Manager),
            }
        }

        // manager+
        CancelOrderItemAny | ReturnOrderItem
        | TransferSession | MergeSessions | SplitSession
        | ApplyDiscountLarge | ViewLiveReports | EditMenu => {
            allow_at(Role::Manager)
        }

        // owner only
        RunEod | EditRecipes | EditStaff | EditSettings => allow_at(Role::Owner),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Action::*;
    use Role::*;

    fn ctx() -> PolicyCtx { PolicyCtx::default() }

    macro_rules! assert_allow {
        ($act:expr, $role:expr) => {
            assert_eq!(check($act, $role, ctx()), Decision::Allow,
                "expected Allow for {:?} as {:?}", $act, $role);
        };
        ($act:expr, $role:expr, $c:expr) => {
            assert_eq!(check($act, $role, $c), Decision::Allow,
                "expected Allow for {:?} as {:?}", $act, $role);
        };
    }
    macro_rules! assert_override {
        ($act:expr, $role:expr, $min:expr) => {
            assert_eq!(check($act, $role, ctx()), Decision::OverrideRequired($min),
                "expected OverrideRequired({:?}) for {:?} as {:?}", $min, $act, $role);
        };
    }

    #[test]
    fn unrestricted_actions_allow_for_staff() {
        for a in [OpenSession, PlaceOrder, ListRooms, ListTables, ListProducts,
                  ListActiveSessions, GetSession] {
            assert_allow!(a, Staff);
        }
    }

    #[test]
    fn cashier_required_actions() {
        for a in [CloseSession, TakePayment] {
            assert_override!(a, Staff, Cashier);
            assert_allow!(a, Cashier);
            assert_allow!(a, Manager);
            assert_allow!(a, Owner);
        }
    }

    #[test]
    fn manager_required_actions() {
        for a in [CancelOrderItemAny, ReturnOrderItem, TransferSession,
                  MergeSessions, SplitSession, ApplyDiscountLarge,
                  ViewLiveReports, EditMenu] {
            assert_override!(a, Staff, Manager);
            assert_override!(a, Cashier, Manager);
            assert_allow!(a, Manager);
            assert_allow!(a, Owner);
        }
    }

    #[test]
    fn owner_required_actions() {
        for a in [RunEod, EditRecipes, EditStaff, EditSettings] {
            assert_override!(a, Manager, Owner);
            assert_allow!(a, Owner);
        }
    }

    #[test]
    fn small_discount_within_threshold_for_cashier() {
        let c = PolicyCtx { discount_pct: Some(10), discount_threshold_pct: 10, ..ctx() };
        assert_allow!(ApplyDiscountSmall, Cashier, c);
    }

    #[test]
    fn small_discount_above_threshold_requires_manager() {
        let c = PolicyCtx { discount_pct: Some(11), discount_threshold_pct: 10, ..ctx() };
        assert_eq!(check(ApplyDiscountSmall, Cashier, c),
                   Decision::OverrideRequired(Role::Manager));
    }

    #[test]
    fn cancel_self_within_grace_allowed_for_staff() {
        let c = PolicyCtx { is_self: true, within_cancel_grace: true, ..ctx() };
        assert_allow!(CancelOrderItemSelf, Staff, c);
    }

    #[test]
    fn cancel_self_outside_grace_requires_manager() {
        let c = PolicyCtx { is_self: true, within_cancel_grace: false, ..ctx() };
        assert_override!(CancelOrderItemSelf, Staff, Manager);
    }

    #[test]
    fn cancel_other_requires_manager_even_within_grace() {
        let c = PolicyCtx { is_self: false, within_cancel_grace: true, ..ctx() };
        assert_override!(CancelOrderItemSelf, Staff, Manager);
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib acl::policy`
Expected: 9 tests pass.

- [ ] **Step 3: Commit**

```bash
git add apps/cashier/src-tauri/src/acl/policy.rs
git commit -m "feat(cashier): ACL policy::check with role hierarchy and override flow"
```

---

## Task 9: `staff` CRUD on master.rs

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: Add Staff struct + methods**

Append to `apps/cashier/src-tauri/src/store/master.rs`:
```rust
use crate::acl::Role;

#[derive(Debug, Clone)]
pub struct Staff {
    pub id: i64,
    pub name: String,
    pub pin_hash: String,
    pub role: Role,
    pub team: Option<String>,
}

impl Master {
    /// Insert a staff row. Returns the new id.
    pub fn create_staff(
        &self,
        name: &str,
        pin_hash: &str,
        role: Role,
        team: Option<&str>,
    ) -> AppResult<i64> {
        self.conn.execute(
            "INSERT INTO staff(name, pin_hash, role, team, created_at)
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![name, pin_hash, role.as_str(), team, now_ms()],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn get_staff(&self, id: i64) -> AppResult<Option<Staff>> {
        Ok(self
            .conn
            .query_row(
                "SELECT id, name, pin_hash, role, team FROM staff WHERE id = ?1",
                params![id],
                row_to_staff,
            )
            .optional()?)
    }

    /// List all staff, ordered by id.
    pub fn list_staff(&self) -> AppResult<Vec<Staff>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, pin_hash, role, team FROM staff ORDER BY id ASC",
        )?;
        let rows = stmt
            .query_map([], row_to_staff)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

fn row_to_staff(r: &rusqlite::Row<'_>) -> rusqlite::Result<Staff> {
    let role_str: String = r.get(3)?;
    let role = Role::parse(&role_str).ok_or_else(|| {
        rusqlite::Error::FromSqlConversionFailure(
            3,
            rusqlite::types::Type::Text,
            format!("bad role: {role_str}").into(),
        )
    })?;
    Ok(Staff {
        id: r.get(0)?,
        name: r.get(1)?,
        pin_hash: r.get(2)?,
        role,
        team: r.get(4)?,
    })
}

#[cfg(test)]
mod staff_tests {
    use super::*;

    #[test]
    fn create_get_staff() {
        let m = Master::open_in_memory().unwrap();
        let id = m.create_staff("Alice", "hash1", Role::Cashier, Some("A")).unwrap();
        let s = m.get_staff(id).unwrap().unwrap();
        assert_eq!(s.name, "Alice");
        assert_eq!(s.role, Role::Cashier);
        assert_eq!(s.team.as_deref(), Some("A"));
    }

    #[test]
    fn list_staff_empty() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.list_staff().unwrap().is_empty());
    }

    #[test]
    fn list_staff_ordered() {
        let m = Master::open_in_memory().unwrap();
        m.create_staff("Bob", "h", Role::Owner, None).unwrap();
        m.create_staff("Cara", "h", Role::Manager, None).unwrap();
        let v = m.list_staff().unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].name, "Bob");
        assert_eq!(v[1].name, "Cara");
    }

    #[test]
    fn missing_staff_returns_none() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.get_staff(999).unwrap().is_none());
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib store::master`
Expected: 6 prior + 4 new = 10 pass.

- [ ] **Step 3: Commit**

```bash
git add apps/cashier/src-tauri/src/store/master.rs
git commit -m "feat(cashier): Staff CRUD on master.db"
```

---

## Task 10: `idempotency` table + service

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0002_idempotency.sql`
- Create: `apps/cashier/src-tauri/src/services/idempotency.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs`

- [ ] **Step 1: Migration**

Create `apps/cashier/src-tauri/src/store/migrations/master/0002_idempotency.sql`:
```sql
CREATE TABLE idempotency_key (
  key           TEXT PRIMARY KEY,
  command       TEXT NOT NULL,
  result_json   TEXT NOT NULL,
  created_at    INTEGER NOT NULL
);
```

- [ ] **Step 2: Service**

Create `apps/cashier/src-tauri/src/services/idempotency.rs`:
```rust
use crate::error::{AppError, AppResult};
use crate::store::master::Master;

pub enum Outcome<T> {
    Inserted(T),       // first time; result was just stored
    Cached(String),    // already-seen key; cached JSON returned
}

/// Run `f`, store its serialized result under `key`. If `key` already exists
/// (UNIQUE conflict), do not call `f` — return the cached result.
pub fn run<T, F>(
    master: &Master,
    key: &str,
    command: &str,
    now_ms: i64,
    f: F,
) -> AppResult<Outcome<T>>
where
    F: FnOnce() -> AppResult<T>,
    T: serde::Serialize,
{
    if let Some(cached) = master.get_idempotency(key)? {
        return Ok(Outcome::Cached(cached));
    }
    let result = f()?;
    let json = serde_json::to_string(&result)
        .map_err(|e| AppError::Internal(format!("idempotency serialize: {e}")))?;
    master.put_idempotency(key, command, &json, now_ms)?;
    Ok(Outcome::Inserted(result))
}
```

- [ ] **Step 3: Master helpers**

Add to `apps/cashier/src-tauri/src/store/master.rs`:
```rust
impl Master {
    pub fn get_idempotency(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self
            .conn
            .query_row(
                "SELECT result_json FROM idempotency_key WHERE key = ?1",
                params![key],
                |r| r.get::<_, String>(0),
            )
            .optional()?)
    }

    /// Insert; on conflict do nothing (caller treats absence of error as "stored or already-stored").
    pub fn put_idempotency(
        &self,
        key: &str,
        command: &str,
        result_json: &str,
        now_ms: i64,
    ) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO idempotency_key(key, command, result_json, created_at)
             VALUES(?1, ?2, ?3, ?4) ON CONFLICT(key) DO NOTHING",
            params![key, command, result_json, now_ms],
        )?;
        Ok(())
    }
}
```

- [ ] **Step 4: Wire into `services/mod.rs`**

```rust
pub mod day_key;
pub mod event_service;
pub mod idempotency;
```

- [ ] **Step 5: Tests**

Append to `apps/cashier/src-tauri/src/services/idempotency.rs`:
```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_call_inserts() {
        let m = Master::open_in_memory().unwrap();
        let r = run(&m, "k1", "cmd", 1, || Ok(42i64)).unwrap();
        match r {
            Outcome::Inserted(v) => assert_eq!(v, 42),
            Outcome::Cached(_) => panic!("expected Inserted"),
        }
    }

    #[test]
    fn second_call_returns_cached_without_running_f() {
        let m = Master::open_in_memory().unwrap();
        run(&m, "k1", "cmd", 1, || Ok::<i64, AppError>(42)).unwrap();
        let r = run(&m, "k1", "cmd", 2, || -> AppResult<i64> {
            panic!("f must not be called on cached key")
        }).unwrap();
        match r {
            Outcome::Cached(json) => assert_eq!(json, "42"),
            Outcome::Inserted(_) => panic!("expected Cached"),
        }
    }
}
```

- [ ] **Step 6: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib services::idempotency && cargo test --test migrations_integration`
Expected: 2 + 4 = 6 pass.

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "feat(cashier): idempotency_key table + service"
```

---

## Task 11: Writer-side validation

**Files:**
- Create: `apps/cashier/src-tauri/src/services/validation.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs`

- [ ] **Step 1: Implement validators**

Create `apps/cashier/src-tauri/src/services/validation.rs`:
```rust
use crate::domain::event::DomainEvent;
use crate::domain::{order, payment, session};
use crate::error::{AppError, AppResult};
use crate::services::event_service::EventService;
use crate::store::events::EventStore;

/// Reject writes that violate aggregate invariants. Caller folds the
/// current state via `EventService::read_decrypted` and passes here.
///
/// Returns Ok(()) if the event is valid in the given context.
pub fn validate(
    svc: &EventService,
    events: &EventStore,
    aggregate_id: &str,
    ev: &DomainEvent,
) -> AppResult<()> {
    match ev {
        DomainEvent::SessionClosed { .. } => {
            let evs = decrypt_aggregate(svc, events, aggregate_id)?;
            let s = session::fold(aggregate_id, &evs)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != session::SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session {aggregate_id} status {:?}, cannot close", s.status
                )));
            }
        }
        DomainEvent::OrderItemCancelled { order_id, item_index, .. } => {
            let evs = decrypt_aggregate(svc, events, order_id)?;
            let o = order::fold(order_id, &evs)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() {
                return Err(AppError::Validation(format!(
                    "item_index {item_index} out of bounds (len {})", o.items.len()
                )));
            }
            if o.items[*item_index].cancelled {
                return Err(AppError::Conflict("item already cancelled".into()));
            }
        }
        DomainEvent::OrderItemReturned { order_id, item_index, qty, .. } => {
            let evs = decrypt_aggregate(svc, events, order_id)?;
            let o = order::fold(order_id, &evs)
                .ok_or_else(|| AppError::Validation("order not placed".into()))?;
            if *item_index >= o.items.len() {
                return Err(AppError::Validation(format!(
                    "item_index {item_index} out of bounds (len {})", o.items.len()
                )));
            }
            let it = &o.items[*item_index];
            let remaining = it.spec.qty - it.returned_qty;
            if *qty <= 0 || *qty > remaining {
                return Err(AppError::Validation(format!(
                    "return qty {qty} invalid (remaining {remaining})"
                )));
            }
        }
        DomainEvent::PaymentTaken { session_id, .. } => {
            // duplicate payment: aggregate_id for PaymentTaken is the payment event's
            // own aggregate, so we look up by session via list_for_aggregate(session_id)
            let evs = decrypt_aggregate(svc, events, session_id)?;
            if payment::fold(session_id, &evs).is_some() {
                return Err(AppError::Conflict("session already paid".into()));
            }
            // also must be Open
            let s = session::fold(session_id, &evs)
                .ok_or_else(|| AppError::Validation("session not opened".into()))?;
            if s.status != session::SessionStatus::Open {
                return Err(AppError::Conflict(format!(
                    "session status {:?}, cannot take payment", s.status
                )));
            }
        }
        // Open/Place/Transfer/Merge/Split need their own pre-checks but Plan C
        // ships these four invariant guards; the remainder are in the command
        // handlers themselves where they have richer context.
        _ => {}
    }
    Ok(())
}

fn decrypt_aggregate(
    svc: &EventService,
    events: &EventStore,
    aggregate_id: &str,
) -> AppResult<Vec<DomainEvent>> {
    events
        .list_for_aggregate(aggregate_id)?
        .iter()
        .map(|r| svc.read_decrypted(r))
        .collect()
}
```

- [ ] **Step 2: Wire into `services/mod.rs`**

```rust
pub mod day_key;
pub mod event_service;
pub mod idempotency;
pub mod validation;
```

- [ ] **Step 3: Integration tests**

Create `apps/cashier/src-tauri/tests/validation_integration.rs`:
```rust
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::{DomainEvent, OrderItemSpec};
use cashier_lib::services::event_service::EventService;
use cashier_lib::services::validation::validate;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex};

fn rig() -> (
    Arc<Mutex<Master>>,
    Arc<EventStore>,
    EventService,
    Arc<MockClock>,
) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let clock = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let svc = EventService {
        master: master.clone(),
        events: events.clone(),
        kek: kek.clone(),
        clock: clock.clone(),
        cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    (master, events, svc, clock)
}

#[test]
fn double_close_rejected() {
    use cashier_lib::services::event_service::WriteCtx;
    let (_, events, svc, _) = rig();

    svc.write(WriteCtx { aggregate_id: "s", actor_staff: Some(1), at: None },
        &DomainEvent::SessionOpened { room_id: Some(1), table_id: None, opened_by: 1,
                                      customer_label: None, team: None }).unwrap();
    svc.write(WriteCtx { aggregate_id: "s", actor_staff: Some(1), at: None },
        &DomainEvent::SessionClosed { closed_by: 1, reason: None }).unwrap();

    let close_again = DomainEvent::SessionClosed { closed_by: 1, reason: None };
    assert!(validate(&svc, &events, "s", &close_again).is_err());
}

#[test]
fn return_more_than_remaining_rejected() {
    use cashier_lib::services::event_service::WriteCtx;
    let (_, events, svc, _) = rig();

    svc.write(WriteCtx { aggregate_id: "o", actor_staff: Some(1), at: None },
        &DomainEvent::OrderPlaced {
            session_id: "s".into(), order_id: "o".into(),
            items: vec![OrderItemSpec { product_id: 1, qty: 2, unit_price: 1000, note: None }],
        }).unwrap();

    let bad_return = DomainEvent::OrderItemReturned {
        order_id: "o".into(), item_index: 0, qty: 5, reason: None,
    };
    assert!(validate(&svc, &events, "o", &bad_return).is_err());

    let good_return = DomainEvent::OrderItemReturned {
        order_id: "o".into(), item_index: 0, qty: 1, reason: None,
    };
    assert!(validate(&svc, &events, "o", &good_return).is_ok());
}

#[test]
fn duplicate_payment_rejected() {
    use cashier_lib::services::event_service::WriteCtx;
    let (_, events, svc, _) = rig();

    svc.write(WriteCtx { aggregate_id: "s", actor_staff: Some(1), at: None },
        &DomainEvent::SessionOpened { room_id: Some(1), table_id: None, opened_by: 1,
                                      customer_label: None, team: None }).unwrap();
    svc.write(WriteCtx { aggregate_id: "s", actor_staff: Some(1), at: None },
        &DomainEvent::PaymentTaken {
            session_id: "s".into(), subtotal: 100, discount_pct: 0, vat_pct: 8,
            total: 108, method: "cash".into(),
        }).unwrap();

    let dup = DomainEvent::PaymentTaken {
        session_id: "s".into(), subtotal: 999, discount_pct: 0, vat_pct: 0,
        total: 999, method: "card".into(),
    };
    assert!(validate(&svc, &events, "s", &dup).is_err());
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --test validation_integration`
Expected: 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(cashier): writer-side validation (close, cancel, return, payment)"
```

---

## Task 12: `AuthService` + `commands/auth_cmd.rs`

**Files:**
- Create: `apps/cashier/src-tauri/src/auth/service.rs`
- Modify: `apps/cashier/src-tauri/src/auth/mod.rs`
- Create: `apps/cashier/src-tauri/src/commands/mod.rs`
- Create: `apps/cashier/src-tauri/src/commands/auth_cmd.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: `apps/cashier/src-tauri/src/app_state.rs`

- [ ] **Step 1: AuthService**

Create `apps/cashier/src-tauri/src/auth/service.rs`:
```rust
use crate::acl::Role;
use crate::auth::pin;
use crate::auth::token::{self, TokenClaims};
use crate::error::{AppError, AppResult};
use crate::store::master::Master;
use crate::time::Clock;
use std::sync::{Arc, Mutex};

/// 12-hour token TTL.
pub const TOKEN_TTL_MS: i64 = 12 * 60 * 60 * 1000;

pub struct AuthService {
    pub master: Arc<Mutex<Master>>,
    pub clock: Arc<dyn Clock>,
    pub signing_key: Arc<Vec<u8>>,
}

impl AuthService {
    /// Verify PIN against any staff row; on success return a signed token.
    /// Constant-time-ish: walks all staff (a real venue has <50; cost negligible).
    pub fn login(&self, pin: &str) -> AppResult<(String, TokenClaims)> {
        let staff_list = self.master.lock().unwrap().list_staff()?;
        let now = self.clock.now().timestamp_millis();
        for s in staff_list {
            if pin::verify_pin(pin, &s.pin_hash)? {
                let claims = TokenClaims {
                    staff_id: s.id,
                    role: s.role,
                    exp: now + TOKEN_TTL_MS,
                };
                let token = token::sign(&claims, &self.signing_key)?;
                tracing::info!(staff_id = s.id, role = ?s.role, "login ok");
                return Ok((token, claims));
            }
        }
        tracing::warn!("login failed: invalid pin");
        Err(AppError::Unauthorized)
    }

    pub fn verify(&self, token: &str) -> AppResult<TokenClaims> {
        let now = self.clock.now().timestamp_millis();
        token::verify(token, &self.signing_key, now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::pin::hash_pin;
    use crate::time::test_support::MockClock;

    fn rig() -> AuthService {
        let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
        let pin = "1234";
        let hash = hash_pin(pin).unwrap();
        master.lock().unwrap()
            .create_staff("Owner", &hash, Role::Owner, None).unwrap();
        let clock: Arc<dyn Clock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
        let key: Arc<Vec<u8>> = Arc::new(vec![7u8; 32]);
        AuthService { master, clock, signing_key: key }
    }

    #[test]
    fn login_with_valid_pin_succeeds() {
        let svc = rig();
        let (token, claims) = svc.login("1234").unwrap();
        assert_eq!(claims.role, Role::Owner);
        let parsed = svc.verify(&token).unwrap();
        assert_eq!(parsed.staff_id, claims.staff_id);
    }

    #[test]
    fn login_with_invalid_pin_unauthorized() {
        let svc = rig();
        assert!(matches!(svc.login("0000"), Err(AppError::Unauthorized)));
    }

    #[test]
    fn verify_tampered_token_unauthorized() {
        let svc = rig();
        let (mut token, _) = svc.login("1234").unwrap();
        let bytes = unsafe { token.as_bytes_mut() };
        bytes[0] ^= 1;
        assert!(matches!(svc.verify(&token), Err(AppError::Unauthorized)));
    }
}
```

- [ ] **Step 2: Update `auth/mod.rs`**

```rust
pub mod pin;
pub mod service;
pub mod token;

pub use service::AuthService;
```

- [ ] **Step 3: Add to `AppState`**

In `app_state.rs`:
```rust
use crate::auth::AuthService;
// add field:
pub auth: AuthService,
```

In `lib.rs::run()`:
```rust
let auth_signing = Arc::new(bootstrap::load_or_init_auth_signing(&ks)?);
let auth = AuthService {
    master: master.clone(),
    clock: clock.clone(),
    signing_key: auth_signing,
};
// then in app.manage(...): include `auth`
```

- [ ] **Step 4: First Tauri command — login**

Create `apps/cashier/src-tauri/src/commands/mod.rs`:
```rust
pub mod auth_cmd;
```

Create `apps/cashier/src-tauri/src/commands/auth_cmd.rs`:
```rust
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::error::AppError;
use serde::{Deserialize, Serialize};
use tauri::State;

#[derive(Debug, Deserialize)]
pub struct LoginInput {
    pub pin: String,
}

#[derive(Debug, Serialize)]
pub struct LoginOutput {
    pub token: String,
    pub claims: TokenClaims,
}

#[tauri::command]
pub fn login(state: State<'_, AppState>, input: LoginInput) -> Result<LoginOutput, AppError> {
    let (token, claims) = state.auth.login(&input.pin)?;
    Ok(LoginOutput { token, claims })
}

#[tauri::command]
pub fn verify_token(state: State<'_, AppState>, token: String) -> Result<TokenClaims, AppError> {
    state.auth.verify(&token)
}
```

- [ ] **Step 5: Wire commands into Tauri builder**

`AppError` must be `serde::Serialize` for `Result<_, AppError>` to flow over IPC. Update `error.rs`:
```rust
use serde::Serialize;

#[derive(Debug, thiserror::Error, Serialize)]
#[serde(tag = "code", content = "message", rename_all = "snake_case")]
pub enum AppError {
    #[serde(serialize_with = "ser_string")] #[error("db: {0}")] Db(#[from] rusqlite::Error),
    #[error("crypto: {0}")] Crypto(String),
    #[error("keychain: {0}")] Keychain(String),
    #[serde(serialize_with = "ser_string")] #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("not found")] NotFound,
    #[error("validation: {0}")] Validation(String),
    #[error("config: {0}")] Config(String),
    #[error("conflict: {0}")] Conflict(String),
    #[error("internal: {0}")] Internal(String),
    #[error("unauthorized")] Unauthorized,
}

fn ser_string<E: std::fmt::Display, S: serde::Serializer>(e: &E, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_str(&e.to_string())
}
```

In `lib.rs`:
```rust
pub mod commands;

// In run():
tauri::Builder::default()
    .setup(|app| { /* as above */ Ok(()) })
    .invoke_handler(tauri::generate_handler![
        commands::auth_cmd::login,
        commands::auth_cmd::verify_token,
    ])
    .run(...)
```

- [ ] **Step 6: Run tests + cargo check**

Run: `cd apps/cashier/src-tauri && cargo test --lib auth::service && cargo check`
Expected: 3 auth tests pass; main compiles.

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "feat(cashier): AuthService + login/verify_token Tauri commands

AppError now derives Serialize so command results flow over IPC.
Token TTL = 12h. AuthService walks staff rows to verify PIN
(constant-cost is fine for venue-scale: <50 staff)."
```

---

## Task 13: Domain command surface (session/order/payment) via `CommandService`

**Files:**
- Create: `apps/cashier/src-tauri/src/services/command_service.rs`
- Create: `apps/cashier/src-tauri/src/commands/{session_cmd,order_cmd,payment_cmd,catalog_cmd}.rs`
- Modify: `apps/cashier/src-tauri/src/services/mod.rs`
- Modify: `apps/cashier/src-tauri/src/commands/mod.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Modify: `apps/cashier/src-tauri/src/app_state.rs`

This task is large but mechanical. The pattern repeats for each command: parse → ACL → idempotency → validate → write event → return projection.

- [ ] **Step 1: Implement `CommandService`**

Create `apps/cashier/src-tauri/src/services/command_service.rs`:
```rust
use crate::acl::{policy, Action, Role};
use crate::auth::token::TokenClaims;
use crate::domain::event::DomainEvent;
use crate::error::{AppError, AppResult};
use crate::services::event_service::{EventService, WriteCtx};
use crate::services::{idempotency, validation};
use crate::store::events::EventStore;
use crate::store::master::Master;
use crate::time::Clock;
use std::sync::{Arc, Mutex};

pub struct CommandService {
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub event_service: EventService,
    pub clock: Arc<dyn Clock>,
}

impl CommandService {
    /// Run the full command pipeline for a single domain event:
    /// ACL guard → idempotency check (skip if cached) → validation → write.
    /// Returns the cached or fresh result as JSON string.
    pub fn execute<T, F>(
        &self,
        actor: &TokenClaims,
        action: Action,
        ctx: policy::PolicyCtx,
        idempotency_key: &str,
        command_name: &str,
        aggregate_id: &str,
        event: DomainEvent,
        project: F,
    ) -> AppResult<T>
    where
        F: FnOnce(&Self) -> AppResult<T>,
        T: serde::Serialize + serde::de::DeserializeOwned,
    {
        match policy::check(action, actor.role, ctx) {
            policy::Decision::Allow => {}
            policy::Decision::Deny => return Err(AppError::Unauthorized),
            policy::Decision::OverrideRequired(min) => {
                return Err(AppError::Conflict(format!(
                    "override_required: {}", min.as_str()
                )));
            }
        }

        let now = self.clock.now().timestamp_millis();
        let outcome = idempotency::run(
            &self.master.lock().unwrap(),
            idempotency_key,
            command_name,
            now,
            || -> AppResult<T> {
                validation::validate(&self.event_service, &self.events, aggregate_id, &event)?;
                self.event_service.write(
                    WriteCtx {
                        aggregate_id,
                        actor_staff: Some(actor.staff_id),
                        at: None,
                    },
                    &event,
                )?;
                project(self)
            },
        )?;

        match outcome {
            idempotency::Outcome::Inserted(v) => Ok(v),
            idempotency::Outcome::Cached(json) => serde_json::from_str(&json).map_err(|e| {
                AppError::Internal(format!("cached idempotency parse: {e}"))
            }),
        }
    }

    /// Helper: load + project a session.
    pub fn load_session(&self, session_id: &str) -> AppResult<Option<crate::domain::session::SessionState>> {
        let rows = self.events.list_for_aggregate(session_id)?;
        let evs: AppResult<Vec<_>> = rows.iter().map(|r| self.event_service.read_decrypted(r)).collect();
        let evs = evs?;
        Ok(crate::domain::session::fold(session_id, &evs))
    }

    pub fn load_order(&self, order_id: &str) -> AppResult<Option<crate::domain::order::OrderState>> {
        let rows = self.events.list_for_aggregate(order_id)?;
        let evs: AppResult<Vec<_>> = rows.iter().map(|r| self.event_service.read_decrypted(r)).collect();
        let evs = evs?;
        Ok(crate::domain::order::fold(order_id, &evs))
    }
}
```

- [ ] **Step 2: Wire into `services/mod.rs` + `app_state.rs`**

`services/mod.rs`:
```rust
pub mod command_service;
pub mod day_key;
pub mod event_service;
pub mod idempotency;
pub mod validation;
```

`app_state.rs` add field:
```rust
pub commands: command_service::CommandService,
```

`lib.rs::run()` build `CommandService` from existing pieces and include in `app.manage(...)`.

- [ ] **Step 3: Implement command files**

Create the four command files. Each `#[tauri::command]` is a thin shim: parse input, call `state.commands.execute(...)`, return projection.

Create `apps/cashier/src-tauri/src/commands/catalog_cmd.rs`:
```rust
use crate::app_state::AppState;
use crate::error::AppError;
use crate::store::master::Staff;
use serde::Serialize;
use tauri::State;

#[derive(Debug, Serialize)]
pub struct StaffOut { pub id: i64, pub name: String, pub role: String, pub team: Option<String> }

#[tauri::command]
pub fn list_staff(state: State<'_, AppState>) -> Result<Vec<StaffOut>, AppError> {
    let staff = state.master.lock().unwrap().list_staff()?;
    Ok(staff.into_iter().map(|s| StaffOut {
        id: s.id, name: s.name, role: s.role.as_str().into(), team: s.team
    }).collect())
}
```

Create `apps/cashier/src-tauri/src/commands/session_cmd.rs`:
```rust
use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::domain::event::DomainEvent;
use crate::domain::session::SessionState;
use crate::error::AppError;
use serde::Deserialize;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct OpenSessionInput {
    pub token: String,
    pub idempotency_key: String,
    pub room_id: Option<i64>,
    pub table_id: Option<i64>,
    pub customer_label: Option<String>,
    pub team: Option<String>,
}

#[tauri::command]
pub fn open_session(
    state: State<'_, AppState>,
    input: OpenSessionInput,
) -> Result<SessionState, AppError> {
    let actor: TokenClaims = state.auth.verify(&input.token)?;
    let session_id = Uuid::new_v4().to_string();
    let event = DomainEvent::SessionOpened {
        room_id: input.room_id,
        table_id: input.table_id,
        opened_by: actor.staff_id,
        customer_label: input.customer_label,
        team: input.team,
    };
    state.commands.execute(
        &actor,
        Action::OpenSession,
        PolicyCtx::default(),
        &input.idempotency_key,
        "open_session",
        &session_id,
        event,
        |cs| {
            cs.load_session(&session_id)?.ok_or(AppError::NotFound)
        },
    )
}

#[derive(Debug, Deserialize)]
pub struct CloseSessionInput {
    pub token: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub reason: Option<String>,
}

#[tauri::command]
pub fn close_session(
    state: State<'_, AppState>,
    input: CloseSessionInput,
) -> Result<SessionState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let event = DomainEvent::SessionClosed { closed_by: actor.staff_id, reason: input.reason };
    state.commands.execute(
        &actor,
        Action::CloseSession,
        PolicyCtx::default(),
        &input.idempotency_key,
        "close_session",
        &input.session_id,
        event,
        |cs| cs.load_session(&input.session_id)?.ok_or(AppError::NotFound),
    )
}
```

Create `apps/cashier/src-tauri/src/commands/order_cmd.rs`:
```rust
use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::{DomainEvent, OrderItemSpec};
use crate::domain::order::OrderState;
use crate::error::AppError;
use serde::Deserialize;
use tauri::State;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct PlaceOrderInput {
    pub token: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub items: Vec<OrderItemSpec>,
}

#[tauri::command]
pub fn place_order(
    state: State<'_, AppState>,
    input: PlaceOrderInput,
) -> Result<OrderState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let order_id = Uuid::new_v4().to_string();
    let event = DomainEvent::OrderPlaced {
        session_id: input.session_id.clone(),
        order_id: order_id.clone(),
        items: input.items,
    };
    state.commands.execute(
        &actor,
        Action::PlaceOrder,
        PolicyCtx::default(),
        &input.idempotency_key,
        "place_order",
        &order_id,
        event,
        |cs| cs.load_order(&order_id)?.ok_or(AppError::NotFound),
    )
}
```

Create `apps/cashier/src-tauri/src/commands/payment_cmd.rs`:
```rust
use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::DomainEvent;
use crate::domain::session::SessionState;
use crate::error::AppError;
use serde::Deserialize;
use tauri::State;

#[derive(Debug, Deserialize)]
pub struct TakePaymentInput {
    pub token: String,
    pub idempotency_key: String,
    pub session_id: String,
    pub subtotal: i64,
    pub discount_pct: u32,
    pub vat_pct: u32,
    pub total: i64,
    pub method: String,
}

#[tauri::command]
pub fn take_payment(
    state: State<'_, AppState>,
    input: TakePaymentInput,
) -> Result<SessionState, AppError> {
    let actor = state.auth.verify(&input.token)?;
    let event = DomainEvent::PaymentTaken {
        session_id: input.session_id.clone(),
        subtotal: input.subtotal, discount_pct: input.discount_pct,
        vat_pct: input.vat_pct, total: input.total, method: input.method,
    };
    let threshold: u32 = state.master.lock().unwrap()
        .get_setting("discount_threshold_pct")?
        .and_then(|s| s.parse().ok()).unwrap_or(10);
    let action = if input.discount_pct == 0 { Action::TakePayment }
        else if input.discount_pct <= threshold { Action::ApplyDiscountSmall }
        else { Action::ApplyDiscountLarge };
    let ctx = PolicyCtx { discount_pct: Some(input.discount_pct),
                          discount_threshold_pct: threshold,
                          ..PolicyCtx::default() };
    state.commands.execute(
        &actor, action, ctx,
        &input.idempotency_key, "take_payment",
        &input.session_id, event,
        |cs| cs.load_session(&input.session_id)?.ok_or(AppError::NotFound),
    )
}
```

- [ ] **Step 4: Update `commands/mod.rs`**

```rust
pub mod auth_cmd;
pub mod catalog_cmd;
pub mod order_cmd;
pub mod payment_cmd;
pub mod session_cmd;
```

- [ ] **Step 5: Update `lib.rs::invoke_handler`**

```rust
.invoke_handler(tauri::generate_handler![
    commands::auth_cmd::login,
    commands::auth_cmd::verify_token,
    commands::catalog_cmd::list_staff,
    commands::session_cmd::open_session,
    commands::session_cmd::close_session,
    commands::order_cmd::place_order,
    commands::payment_cmd::take_payment,
])
```

- [ ] **Step 6: cargo check + cargo test**

Run: `cd apps/cashier/src-tauri && cargo check && cargo test`
Expected: compiles; existing tests pass; no new tests added in this task (covered by command integration test in Task 14).

- [ ] **Step 7: Commit**

```bash
git add -u
git commit -m "feat(cashier): CommandService + session/order/payment Tauri commands

Each command is a thin shim: parse input, verify token, build event,
delegate to CommandService::execute which runs ACL → idempotency →
validation → write → projection load. Discount tier is computed
from the discount_threshold_pct setting at write time."
```

---

## Task 14: Command integration test

**Files:**
- Create: `apps/cashier/src-tauri/tests/commands_integration.rs`

- [ ] **Step 1: Happy path through CommandService**

Create `apps/cashier/src-tauri/tests/commands_integration.rs`:
```rust
use cashier_lib::acl::{policy::PolicyCtx, Action, Role};
use cashier_lib::auth::pin::hash_pin;
use cashier_lib::auth::AuthService;
use cashier_lib::crypto::Kek;
use cashier_lib::domain::event::{DomainEvent, OrderItemSpec};
use cashier_lib::services::command_service::CommandService;
use cashier_lib::services::event_service::EventService;
use cashier_lib::store::events::EventStore;
use cashier_lib::store::master::Master;
use cashier_lib::time::test_support::MockClock;
use chrono::FixedOffset;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

fn rig() -> (CommandService, AuthService, Arc<MockClock>) {
    let master = Arc::new(Mutex::new(Master::open_in_memory().unwrap()));
    let events = Arc::new(EventStore::open_in_memory().unwrap());
    let kek = Arc::new(Kek::new_random());
    let clock: Arc<MockClock> = Arc::new(MockClock::at_ymd_hms(2026, 4, 27, 12, 0, 0));
    let event_service = EventService {
        master: master.clone(), events: events.clone(), kek: kek.clone(),
        clock: clock.clone(), cutoff_hour: 11,
        tz: FixedOffset::east_opt(7 * 3600).unwrap(),
    };
    let commands = CommandService {
        master: master.clone(), events: events.clone(),
        event_service, clock: clock.clone(),
    };
    let signing = Arc::new(vec![1u8; 32]);
    let auth = AuthService { master: master.clone(), clock: clock.clone(), signing_key: signing };

    let pin_hash = hash_pin("9999").unwrap();
    master.lock().unwrap()
        .create_staff("Owner", &pin_hash, Role::Owner, None).unwrap();
    (commands, auth, clock)
}

#[test]
fn full_command_lifecycle() {
    let (cs, auth, _) = rig();
    let (_, claims) = auth.login("9999").unwrap();

    // Open session
    let session_id = Uuid::new_v4().to_string();
    let opened = cs.execute(
        &claims, Action::OpenSession, PolicyCtx::default(),
        "k1", "open_session", &session_id,
        DomainEvent::SessionOpened {
            room_id: Some(1), table_id: None, opened_by: claims.staff_id,
            customer_label: Some("VIP".into()), team: None,
        },
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();
    assert_eq!(opened.room_id, Some(1));

    // Place order
    let order_id = Uuid::new_v4().to_string();
    cs.execute(
        &claims, Action::PlaceOrder, PolicyCtx::default(),
        "k2", "place_order", &order_id,
        DomainEvent::OrderPlaced {
            session_id: session_id.clone(), order_id: order_id.clone(),
            items: vec![OrderItemSpec { product_id: 1, qty: 2, unit_price: 50_000, note: None }],
        },
        |c| c.load_order(&order_id).map(|o| o.unwrap()),
    ).unwrap();

    // Take payment
    cs.execute(
        &claims, Action::TakePayment, PolicyCtx::default(),
        "k3", "take_payment", &session_id,
        DomainEvent::PaymentTaken {
            session_id: session_id.clone(), subtotal: 100_000,
            discount_pct: 0, vat_pct: 8, total: 108_000, method: "cash".into(),
        },
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();

    // Close
    let closed = cs.execute(
        &claims, Action::CloseSession, PolicyCtx::default(),
        "k4", "close_session", &session_id,
        DomainEvent::SessionClosed { closed_by: claims.staff_id, reason: None },
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();
    assert_eq!(closed.status, cashier_lib::domain::session::SessionStatus::Closed);
}

#[test]
fn idempotent_replay_returns_same_result_without_double_write() {
    let (cs, auth, _) = rig();
    let (_, claims) = auth.login("9999").unwrap();

    let session_id = Uuid::new_v4().to_string();
    let event = DomainEvent::SessionOpened {
        room_id: Some(1), table_id: None, opened_by: claims.staff_id,
        customer_label: None, team: None,
    };

    let first = cs.execute(
        &claims, Action::OpenSession, PolicyCtx::default(),
        "same-key", "open_session", &session_id,
        event.clone(),
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();

    let second = cs.execute(
        &claims, Action::OpenSession, PolicyCtx::default(),
        "same-key", "open_session", &session_id,
        event,
        |c| c.load_session(&session_id).map(|o| o.unwrap()),
    ).unwrap();

    assert_eq!(first, second);
}
```

(Note: `SessionState` and `OrderState` need `PartialEq + Clone + Serialize + Deserialize` for the idempotency test's `assert_eq!` and JSON cache. They already derive `PartialEq + Clone` from Plan B. Add `Serialize + Deserialize` to both — verify and add if missing in `domain/session.rs` and `domain/order.rs`. This is a small forward-fix.)

- [ ] **Step 2: Add Serialize+Deserialize to projection structs**

Modify `apps/cashier/src-tauri/src/domain/session.rs`:
- `SessionStatus`: add `#[derive(Serialize, Deserialize)]` (alongside existing derives), `use serde::{Deserialize, Serialize};`
- `SessionState`: same

Same for `OrderState`, `OrderItem`, `PaymentState`.

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --test commands_integration && cargo test`
Expected: 2 new + all prior pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "test(cashier): command lifecycle + idempotency integration tests

Adds Serialize/Deserialize to projection structs (needed for idempotency
JSON cache and for over-IPC command return values)."
```

---

## Task 15: Final gate

- [ ] **Step 1**: `cd apps/cashier/src-tauri && cargo test`
  Expected: ~85+ tests pass.
- [ ] **Step 2**: `cargo clippy --all-targets -- -D warnings`
- [ ] **Step 3**: `cargo fmt --check` (run `cargo fmt` if needed; commit `chore: cargo fmt` if any change)
- [ ] **Step 4**: `pnpm --filter @tableorder/cashier typecheck`
- [ ] **Step 5**: If any fmt commit:
  ```
  git add -u && git commit -m "chore(cashier): cargo fmt"
  ```

---

## Done

End state:
- Argon2id PIN hash + verify; HMAC-signed bearer tokens; auth signing key in keychain
- Role hierarchy + Action enum + `policy::check` with Allow/Deny/OverrideRequired
- Staff CRUD; idempotency table + service; writer-side validation
- `EventService` with `Arc` fields + `CommandService` orchestration (ACL → idempotency → validate → write → project)
- Tauri commands: `login`, `verify_token`, `list_staff`, `open_session`, `close_session`, `place_order`, `take_payment`
- All commands return `Result<T, AppError>` with `AppError` serde-mapped to `{code, message}`

Ready for **Plan D** (axum HTTP+WS so order tablets can hit the same command surface; `packages/shared` TS client) and **Plan E** (EOD pipeline + reports + print stubs at write sites).

Commands NOT yet implemented (Plan D will add them via the same pattern):
- `transfer_session`, `merge_sessions`, `split_session`
- `cancel_order_item`, `return_order_item`
- `list_active_sessions`, `get_session`
- `list_rooms`, `list_tables`, `list_products`
- `view_live_reports`
