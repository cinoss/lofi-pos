# Foundation Plan E1b — TS Shared Client + Backend Prereqs

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the HTTP+WS surface usable by browsers (the cashier window and the future tablet PWA) AND close the security gaps E1a deferred. End state: a TS client in `packages/shared` exports typed fetch + WS hook; the WebSocket auths via first-message handshake; PIN length enforced; rate-limit on login; `OverrideRequired` returns 403.

**Architecture:** Two halves.

**Backend prereqs (Rust):**
- WS auth: drop the upgrade-time `AuthCtx`; require first WS message to be `{"type":"hello","token":"..."}`; validate before subscribing to broadcast.
- PIN length: validate `>= 6` digits in `auth::pin::hash_pin`.
- `OverrideRequired` → HTTP 403 (was 401).
- Rate-limit `/auth/login` via `governor` crate (per-IP, 10/min, returns 429).
- New `POST /auth/logout` records token JTI to a denylist so explicit lock works.

**TS shared client (`packages/shared`):**
- zod schemas for every request/response/event shape
- `apiClient(baseUrl, getToken)` wrapper — typed fetch that throws `AppError`-shaped errors
- `useEventStream(baseUrl, getToken)` framework-agnostic WS hook (returns observable/iterator over EventNotices)
- TypeScript types exported for use by `apps/cashier` and `apps/web`

**Tech stack additions:**
- Rust: `governor`, `uuid` (already there for v4 — also use for token JTI if not already)
- TS: `zod`

**Spec:** `docs/superpowers/specs/2026-04-27-foundation-design.md` (HTTP API + WS section); design addendum for the new WS first-message handshake captured below.

**Builds on:** A through E1a.

**Out of scope (Plan E1c):**
- `apps/web` tablet PWA
- Cashier React UI rewire (still a `<h1>Cashier</h1>` stub)
- Idle lock UI (frontend timer)
- PWA manifest + service worker

**Out of scope (Plan F):**
- EOD pipeline, print stubs

---

## File Structure

```
apps/cashier/src-tauri/src/
  http/
    error_layer.rs                      # MODIFIED — OverrideRequired → 403
    routes/
      auth.rs                           # MODIFIED — POST /auth/logout
      ws.rs                             # MODIFIED — first-message handshake
  auth/
    pin.rs                              # MODIFIED — min length validation
    token.rs                            # MODIFIED — TokenClaims gains jti (UUID)
    service.rs                          # MODIFIED — login generates jti; verify checks denylist
  store/
    master.rs                           # MODIFIED — token_denylist CRUD
    migrations/master/
      0004_token_denylist.sql           # NEW — denylist table
  http/server.rs                        # MODIFIED — governor rate-limit on /auth/login
packages/shared/                        # NEW
  package.json
  tsconfig.json
  src/
    index.ts                            # public exports
    error.ts                            # AppError envelope type + ApiError class
    schemas/
      common.ts                         # SpotRef, Route, OrderItemSpec, RecipeIngredientSnapshot, EventNotice, NoticeKind
      auth.ts                           # LoginInput/Output, TokenClaims, Role
      session.ts                        # OpenSessionInput, SessionState, etc.
      order.ts                          # PlaceOrderInput, OrderState, RawOrderItem, etc.
      payment.ts                        # TakePaymentInput, PaymentState
      catalog.ts                        # Spot, Product, StaffOut
    client.ts                           # apiClient factory (typed fetch wrapper)
    ws.ts                               # createEventStream (first-message handshake)
tests/
  ws_handshake_integration.rs           # NEW — replaces old WS test; covers first-message auth
  pin_length_test.rs                    # NEW or fold into existing pin tests
  http_integration.rs                   # MODIFIED — OverrideRequired status changes 401→403
```

Decisions:
- **WS first-message handshake** — server upgrades immediately; client must send `{"type":"hello","token":"..."}` as the first frame within 5 seconds. Server validates, replies `{"type":"hello.ok"}` (or `{"type":"error","code":"unauthorized"}` then closes), then begins relaying broadcast events.
- **TokenClaims gains `jti: String`** (UUID v4) — needed for denylist. `#[serde(default = "Uuid::new_v4()...")]` — but defaults can't call functions. Use plain `String` field; old tokens without `jti` get a synthetic one or fail (since old tokens predate this change and no production data exists, fail is fine).
- **Denylist is a master.db table**: `token_denylist(jti TEXT PK, expires_at INTEGER)`. EOD or a periodic job prunes expired rows (Plan F). Each `verify` checks `WHERE jti = ?`.
- **PIN minimum length: 6 digits.** Hard-coded; no setting yet (admin-changeable in a later plan). Migration of existing dev seed PIN "1234" → "123456".
- **`packages/shared` consumed via pnpm workspace** — both `apps/cashier` and (future) `apps/web` add `"@workspace/shared": "workspace:*"`.

---

## Task 1: PIN length enforcement

**Files:** Modify `apps/cashier/src-tauri/src/auth/pin.rs`

- [ ] **Step 1: Add length check in `hash_pin`**

```rust
pub const MIN_PIN_LENGTH: usize = 6;

pub fn hash_pin(pin: &str) -> AppResult<String> {
    if pin.chars().count() < MIN_PIN_LENGTH {
        return Err(AppError::Validation(format!(
            "pin must be at least {MIN_PIN_LENGTH} characters"
        )));
    }
    /* existing Argon2id hashing */
}
```

`verify_pin` does NOT check length — accepts any input PIN since it might match a hash from a prior schema. Only `hash_pin` (admin creates staff or rotates PIN) enforces.

- [ ] **Step 2: Update unit tests**

The 4 existing tests in `pin.rs` use `"1234"`. Update to `"123456"`. Add one new test: `hash_pin_rejects_short`.

- [ ] **Step 3: Find and update other test fixtures**

`grep -rn 'hash_pin("' apps/cashier/src-tauri/` — find every `hash_pin("...")` call. Update PINs to 6+ digits.
`grep -rn '"1234"\|"9999"\|"1111"\|"5555"\|"0000"' apps/cashier/src-tauri/tests/` — find login/PIN string literals.

Common patterns to replace:
- `"1234"` → `"123456"`
- `"9999"` → `"999999"`
- `"1111"` → `"111111"`
- `"5555"` → `"555555"`
- `"0000"` → `"000000"`

Each test will need both the `hash_pin("XXXXXX")` call AND the corresponding `auth.login("XXXXXX")` updated.

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all 167 still pass + 1 new = 168.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(cashier): enforce minimum PIN length 6 in hash_pin

Closes the dev-seed weak-PIN risk flagged in Plan E1a final review.
verify_pin unchanged (accepts any input — only hash_pin gates new
PINs). All test fixtures bumped from 4-digit to 6-digit PINs."
```

---

## Task 2: `OverrideRequired` → HTTP 403

**Files:**
- Modify: `apps/cashier/src-tauri/src/http/error_layer.rs`
- Modify: `apps/cashier/src-tauri/tests/http_integration.rs` (assertion update)

- [ ] **Step 1: Change mapping**

In `error_layer.rs`:
```rust
let status = match &self.0 {
    AppError::Validation(_)              => StatusCode::BAD_REQUEST,
    AppError::Conflict(_)                => StatusCode::CONFLICT,
    AppError::NotFound                   => StatusCode::NOT_FOUND,
    AppError::Unauthorized               => StatusCode::UNAUTHORIZED,
    AppError::OverrideRequired(_)        => StatusCode::FORBIDDEN,   // CHANGED
    /* rest unchanged → 500 */
};
```

- [ ] **Step 2: Update unit test**

In `error_layer.rs::tests::override_required_includes_role_in_message`:
```rust
assert_eq!(r.status(), StatusCode::FORBIDDEN);   // was UNAUTHORIZED
```

- [ ] **Step 3: Update HTTP integration test**

In `tests/http_integration.rs::http_override_required_returns_403_with_message` (rename the test):
```rust
assert_eq!(resp.status(), 403);   // was 401
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: still pass.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "fix(cashier): OverrideRequired returns 403 (not 401)

HTTP convention: 401 = not authenticated; 403 = authenticated but
not authorized. Override required is the latter. Frontend interceptor
can now route 401→login, 403+code='override_required'→manager-PIN."
```

---

## Task 3: TokenClaims.jti + denylist schema

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0004_token_denylist.sql`
- Modify: `apps/cashier/src-tauri/src/auth/token.rs`
- Modify: `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: Migration**

```sql
-- 0004_token_denylist.sql
CREATE TABLE token_denylist (
  jti        TEXT PRIMARY KEY,
  expires_at INTEGER NOT NULL,           -- unix-ms; rows past expiry can be pruned
  revoked_at INTEGER NOT NULL
);

CREATE INDEX idx_token_denylist_expires ON token_denylist(expires_at);
```

- [ ] **Step 2: Add `jti` to TokenClaims**

In `token.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenClaims {
    pub staff_id: i64,
    pub role: Role,
    #[serde(default = "default_role")]   // already there
    pub iat: i64,
    pub exp: i64,
    /// Token id — UUID v4. Used for revocation/denylist lookup.
    /// Old tokens without `jti` get a synthetic empty string and will be
    /// rejected at verify if denylist is consulted strictly.
    #[serde(default)]
    pub jti: String,
}
```

(Existing `iat` field already has `#[serde(default)]` from Plan C polish; the same shape works for `jti`.)

- [ ] **Step 3: Master CRUD**

```rust
impl Master {
    pub fn put_token_denylist(&self, jti: &str, expires_at: i64, now_ms: i64) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO token_denylist(jti, expires_at, revoked_at)
             VALUES(?1, ?2, ?3) ON CONFLICT(jti) DO NOTHING",
            params![jti, expires_at, now_ms],
        )?;
        Ok(())
    }

    pub fn is_token_denylisted(&self, jti: &str) -> AppResult<bool> {
        Ok(self.conn.query_row(
            "SELECT 1 FROM token_denylist WHERE jti = ?1",
            params![jti], |_| Ok(true),
        ).optional()?.unwrap_or(false))
    }
}
```

Add 3 unit tests in `mod tests`: `put_then_check`, `not_present_returns_false`, `idempotent_put`.

- [ ] **Step 4: Update existing tests**

Migration table list test (`migrations_integration::expected_tables_exist_after_migration`) — add `token_denylist`.

`token.rs` tests: add `jti: "test-jti".into()` to `claims()` helper. The `pre_wave2_token_defaults_role_to_staff` test should also assert `parsed.jti == ""` (default).

- [ ] **Step 5: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 168 + 3 = 171.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "feat(cashier): TokenClaims.jti + master.token_denylist CRUD

Foundation for token revocation. jti generated at login (Task 4);
verify checks denylist (Task 4). Migration 0004 adds the table."
```

---

## Task 4: AuthService — generate jti at login; check denylist on verify

**Files:** Modify `apps/cashier/src-tauri/src/auth/service.rs`

- [ ] **Step 1: Generate jti in login**

```rust
use uuid::Uuid;

pub fn login(&self, pin: &str) -> AppResult<(String, TokenClaims)> {
    let staff_list = self.master.lock().unwrap().list_staff()?;
    let now = self.clock.now().timestamp_millis();
    for s in staff_list {
        if pin::verify_pin(pin, &s.pin_hash)? {
            let claims = TokenClaims {
                staff_id: s.id, role: s.role,
                iat: now, exp: now + TOKEN_TTL_MS,
                jti: Uuid::new_v4().to_string(),     // NEW
            };
            let token = token::sign(&claims, &self.signing_key)?;
            tracing::info!(staff_id = s.id, jti = %claims.jti, "login ok");
            return Ok((token, claims));
        }
    }
    tracing::warn!("login failed: invalid pin");
    Err(AppError::Unauthorized)
}
```

- [ ] **Step 2: Check denylist in verify**

```rust
pub fn verify(&self, token: &str) -> AppResult<TokenClaims> {
    let now = self.clock.now().timestamp_millis();
    let claims = token::verify(token, &self.signing_key, now)?;
    if !claims.jti.is_empty()
        && self.master.lock().unwrap().is_token_denylisted(&claims.jti)?
    {
        tracing::warn!(jti = %claims.jti, "token rejected (denylisted)");
        return Err(AppError::Unauthorized);
    }
    Ok(claims)
}
```

- [ ] **Step 3: Add `revoke` helper**

```rust
pub fn revoke(&self, claims: &TokenClaims) -> AppResult<()> {
    if claims.jti.is_empty() { return Ok(()); }     // pre-jti tokens — nothing to track
    let now = self.clock.now().timestamp_millis();
    self.master.lock().unwrap()
        .put_token_denylist(&claims.jti, claims.exp, now)
}
```

- [ ] **Step 4: Tests**

Add 3 tests:
```rust
#[test]
fn login_generates_unique_jti() {
    let svc = rig();
    let (_, c1) = svc.login("123456").unwrap();
    let (_, c2) = svc.login("123456").unwrap();
    assert_ne!(c1.jti, c2.jti);
    assert!(!c1.jti.is_empty());
}

#[test]
fn revoked_token_fails_verify() {
    let svc = rig();
    let (token, claims) = svc.login("123456").unwrap();
    svc.revoke(&claims).unwrap();
    assert!(matches!(svc.verify(&token), Err(AppError::Unauthorized)));
}

#[test]
fn revoke_is_idempotent() {
    let svc = rig();
    let (_, claims) = svc.login("123456").unwrap();
    svc.revoke(&claims).unwrap();
    svc.revoke(&claims).unwrap();   // should not error
}
```

- [ ] **Step 5: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 171 + 3 = 174.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "feat(cashier): AuthService generates jti; verify checks denylist; revoke helper"
```

---

## Task 5: `POST /auth/logout` endpoint

**Files:** Modify `apps/cashier/src-tauri/src/http/routes/auth.rs`

- [ ] **Step 1: Add route**

```rust
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/me", get(me))
}

async fn logout(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
) -> Result<axum::http::StatusCode, AppErrorResponse> {
    let auth = state.auth.clone();
    tokio::task::spawn_blocking(move || auth.revoke(&claims)).await
        .map_err(|e| AppErrorResponse(crate::error::AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(axum::http::StatusCode::NO_CONTENT)
}
```

- [ ] **Step 2: HTTP integration test**

In `tests/http_integration.rs`, add `http_logout_revokes_token`:
```rust
#[tokio::test]
async fn http_logout_revokes_token() {
    // login → get token → POST /auth/logout → re-use token → 401
}
```

- [ ] **Step 3: Run + commit**

```bash
cargo test
git add -u
git commit -m "feat(cashier): POST /auth/logout revokes the calling token"
```

---

## Task 6: WS first-message handshake

**Files:** Modify `apps/cashier/src-tauri/src/http/routes/ws.rs`

The current WS handler uses `AuthCtx` extractor on the upgrade request. Browsers can't set Authorization header on `WebSocket(url)` — this fails in real frontends. Switch to first-message handshake: upgrade unconditionally, then require `{"type":"hello","token":"..."}` as the first frame within 5 seconds.

- [ ] **Step 1: Refactor `ws_handler`**

```rust
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::http::broadcast::EventNotice;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

#[derive(Deserialize)]
#[serde(tag = "type")]
enum ClientHello {
    #[serde(rename = "hello")]
    Hello { token: String },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ServerMsg {
    HelloOk,
    Error { code: String, message: Option<String> },
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    ws: WebSocketUpgrade,
) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: Arc<AppState>) {
    // First-message handshake — 5 second timeout.
    let claims = match handshake(&mut socket, &state).await {
        Ok(c) => c,
        Err(reason) => {
            let _ = send_error(&mut socket, "unauthorized", Some(reason)).await;
            return;
        }
    };

    let _ = send_json(&mut socket, &ServerMsg::HelloOk).await;
    tracing::info!(staff_id = claims.staff_id, "ws subscriber attached");

    let mut rx = state.broadcast_tx.subscribe();
    while let Ok(notice) = rx.recv().await {
        if send_json(&mut socket, &notice).await.is_err() { break; }
    }
}

async fn handshake(socket: &mut WebSocket, state: &Arc<AppState>) -> Result<TokenClaims, String> {
    let msg = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await
        .map_err(|_| "handshake timeout".to_string())?
        .ok_or_else(|| "client closed".to_string())?
        .map_err(|e| format!("recv error: {e}"))?;

    let text = match msg {
        Message::Text(s) => s,
        _ => return Err("expected text hello".into()),
    };

    let hello: ClientHello = serde_json::from_str(&text)
        .map_err(|e| format!("bad hello json: {e}"))?;

    let ClientHello::Hello { token } = hello;
    state.auth.verify(&token).map_err(|_| "invalid token".to_string())
}

async fn send_json<T: Serialize>(socket: &mut WebSocket, v: &T) -> Result<(), axum::Error> {
    let json = serde_json::to_string(v).unwrap_or_else(|_| "{}".into());
    socket.send(Message::Text(json)).await
}

async fn send_error(socket: &mut WebSocket, code: &str, message: Option<String>) -> Result<(), axum::Error> {
    send_json(socket, &ServerMsg::Error {
        code: code.into(),
        message,
    }).await
}
```

- [ ] **Step 2: Replace WS integration test**

The existing `http_ws_receives_event_notice_on_write` test in `http_integration.rs` connects with Authorization header (works for `tokio_tungstenite`, won't work for browsers). Update it to use the new first-message handshake:

```rust
// After connecting, send hello, expect hello_ok, then proceed.
let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();

// Send hello
ws.send(Message::Text(format!(r#"{{"type":"hello","token":"{token}"}}"#))).await.unwrap();

// Expect hello_ok
let reply = ws.next().await.unwrap().unwrap();
let reply_text = reply.into_text().unwrap();
assert!(reply_text.contains("hello_ok"));

// Then trigger an HTTP write and expect EventNotice
// ... same as before ...
```

Add a second test: `http_ws_unauthenticated_handshake_rejected`:
```rust
let (mut ws, _) = tokio_tungstenite::connect_async(format!("ws://{addr}/ws")).await.unwrap();
ws.send(Message::Text(r#"{"type":"hello","token":"junk"}"#.into())).await.unwrap();
let reply = ws.next().await.unwrap().unwrap();
let reply_text = reply.into_text().unwrap();
assert!(reply_text.contains("\"unauthorized\""));
```

- [ ] **Step 3: Run + commit**

```bash
cargo test
git add -u
git commit -m "feat(cashier): WS auth via first-message handshake (browser-compatible)

Browsers can't set Authorization header on WebSocket(url) — drop the
upgrade-time AuthCtx and require {type:hello, token} as the first
frame within 5s. Server replies {type:hello_ok} on success or
{type:error, code:unauthorized} + close on failure."
```

---

## Task 7: Rate-limit `/auth/login` via governor

**Files:**
- Modify: `apps/cashier/src-tauri/Cargo.toml`
- Modify: `apps/cashier/src-tauri/src/http/server.rs`

- [ ] **Step 1: Add deps**

```toml
governor = "0.6"
```

- [ ] **Step 2: Build a small per-IP keyed limiter middleware**

`governor` provides `RateLimiter`; we wrap it in a tower layer manually.

In `http/server.rs`:
```rust
use axum::http::{Request, StatusCode};
use axum::middleware::{self, Next};
use axum::response::Response;
use governor::clock::DefaultClock;
use governor::state::{InMemoryState, NotKeyed, RateLimiter};
use governor::Quota;
use std::net::IpAddr;
use std::num::NonZeroU32;
use std::sync::Arc;

pub type LoginLimiter = RateLimiter<IpAddr, dashmap::DashMap<IpAddr, InMemoryState>, DefaultClock>;

pub fn login_limiter() -> Arc<LoginLimiter> {
    let quota = Quota::per_minute(NonZeroU32::new(10).unwrap());
    Arc::new(RateLimiter::dashmap(quota))
}

pub async fn login_rate_limit(
    State(limiter): State<Arc<LoginLimiter>>,
    axum::extract::ConnectInfo(addr): axum::extract::ConnectInfo<std::net::SocketAddr>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if limiter.check_key(&addr.ip()).is_err() {
        return (StatusCode::TOO_MANY_REQUESTS,
                axum::Json(serde_json::json!({"code":"rate_limited","message":"too many login attempts"})))
            .into_response();
    }
    next.run(request).await
}
```

(Verify `governor`'s API: `RateLimiter::dashmap` returns the keyed type. May need to wire DashMap directly. If `governor`'s API doesn't fit cleanly, use `tower_governor` crate which provides the layer pre-built.)

If `tower_governor = "0.4"` is simpler:
```toml
tower_governor = "0.4"
```
```rust
use tower_governor::governor::GovernorConfigBuilder;
use tower_governor::GovernorLayer;

let governor_conf = GovernorConfigBuilder::default()
    .per_second(6)         // 10/min ≈ one per 6s
    .burst_size(3)
    .finish().unwrap();

let login_only = Router::new()
    .merge(crate::http::routes::auth::router())
    .layer(GovernorLayer { config: Arc::new(governor_conf) });
```

`tower_governor` returns 429 directly. Recommend that path.

- [ ] **Step 3: Need ConnectInfo**

axum's `into_make_service_with_connect_info::<SocketAddr>()` gives handlers access to client IP. Update `serve`:
```rust
axum::serve(listener, router.into_make_service_with_connect_info::<SocketAddr>()).await?;
```

- [ ] **Step 4: HTTP integration test**

```rust
#[tokio::test]
async fn http_login_rate_limit_returns_429() {
    let (addr, _state) = rig().await;
    let client = reqwest::Client::new();
    let url = format!("http://{addr}/auth/login");
    let bad = serde_json::json!({"pin":"000000"});
    let mut got_429 = false;
    for _ in 0..20 {
        let r = client.post(&url).json(&bad).send().await.unwrap();
        if r.status() == 429 { got_429 = true; break; }
    }
    assert!(got_429, "expected 429 within 20 rapid attempts");
}
```

- [ ] **Step 5: Update README**

Soften → make accurate. The "currently a TODO" line in `apps/cashier/README.md` becomes:
```markdown
- **Rate limit on `/auth/login`** via `tower_governor` — 10 attempts/IP/min,
  burst 3. Returns 429 on exceed. Argon2id PIN hashing makes online
  bruteforce of a 6-digit PIN take years even without rate limiting; the
  rate limit cuts that to "infeasible."
```

- [ ] **Step 6: Run + commit**

```bash
cargo test
git add -u
git commit -m "feat(cashier): rate-limit /auth/login via tower_governor (10/IP/min, 429)

Closes the rate-limit TODO from Plan E1a. Combined with PIN length
enforcement (Task 1) and Argon2id, online bruteforce is infeasible
on a properly segmented staff network."
```

---

## Task 8: Backend gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — ~178 pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean

---

## Task 9: `packages/shared` skeleton

**Files:**
- Create: `packages/shared/package.json`
- Create: `packages/shared/tsconfig.json`
- Create: `packages/shared/src/index.ts`

- [ ] **Step 1: package.json**

```json
{
  "name": "@workspace/shared",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "./src/index.ts",
  "types": "./src/index.ts",
  "exports": {
    ".": "./src/index.ts"
  },
  "scripts": {
    "typecheck": "tsc --noEmit",
    "lint": "eslint ."
  },
  "dependencies": {
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "typescript": "5.9.3",
    "@types/ws": "^8.5.0"
  }
}
```

- [ ] **Step 2: tsconfig.json**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "skipLibCheck": true,
    "noEmit": true,
    "esModuleInterop": true
  },
  "include": ["src"]
}
```

- [ ] **Step 3: empty index.ts**

```ts
export * from "./error";
export * from "./schemas/common";
export * from "./schemas/auth";
export * from "./schemas/session";
export * from "./schemas/order";
export * from "./schemas/payment";
export * from "./schemas/catalog";
export * from "./client";
export * from "./ws";
```

(Compile errors expected until subsequent tasks fill the modules.)

- [ ] **Step 4: pnpm install**

Run: `pnpm install` from repo root.
Expected: workspace recognizes `@workspace/shared`. New workspace dependency.

- [ ] **Step 5: Commit**

```bash
git add packages/shared pnpm-lock.yaml pnpm-workspace.yaml
git commit -m "feat(shared): scaffold @workspace/shared (zod-based TS client package)"
```

---

## Task 10: zod schemas — common types + auth

**Files:**
- Create: `packages/shared/src/error.ts`
- Create: `packages/shared/src/schemas/common.ts`
- Create: `packages/shared/src/schemas/auth.ts`

- [ ] **Step 1: error.ts**

```ts
import { z } from "zod";

export const AppErrorEnvelope = z.object({
  code: z.string(),
  message: z.string().optional(),
});
export type AppErrorEnvelope = z.infer<typeof AppErrorEnvelope>;

/// Thrown by apiClient on non-2xx responses; carries the parsed envelope.
export class ApiError extends Error {
  readonly code: string;
  readonly status: number;
  readonly envelope: AppErrorEnvelope;

  constructor(status: number, envelope: AppErrorEnvelope) {
    super(envelope.message ?? envelope.code);
    this.code = envelope.code;
    this.status = status;
    this.envelope = envelope;
  }

  isOverrideRequired(): boolean { return this.code === "override_required"; }
  isUnauthorized(): boolean { return this.code === "unauthorized"; }
  isValidation(): boolean { return this.code === "validation"; }
  isConflict(): boolean { return this.code === "conflict"; }
}
```

- [ ] **Step 2: schemas/common.ts** — mirror of Rust types

```ts
import { z } from "zod";

export const Role = z.enum(["staff", "cashier", "manager", "owner"]);
export type Role = z.infer<typeof Role>;

export const Route = z.enum(["kitchen", "bar", "none"]);
export type Route = z.infer<typeof Route>;

export const SpotKind = z.enum(["room", "table"]);
export type SpotKind = z.infer<typeof SpotKind>;

export const SpotRef = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("room"),
    id: z.number().int(),
    name: z.string(),
    hourly_rate: z.number().int(),
  }),
  z.object({
    kind: z.literal("table"),
    id: z.number().int(),
    name: z.string(),
    room_id: z.number().int().nullable(),
    room_name: z.string().nullable(),
  }),
]);
export type SpotRef = z.infer<typeof SpotRef>;

export const RecipeIngredientSnapshot = z.object({
  ingredient_id: z.number().int(),
  ingredient_name: z.string(),
  qty: z.number(),
  unit: z.string(),
});
export type RecipeIngredientSnapshot = z.infer<typeof RecipeIngredientSnapshot>;

export const OrderItemSpec = z.object({
  product_id: z.number().int(),
  product_name: z.string(),
  qty: z.number().int(),
  unit_price: z.number().int(),
  note: z.string().nullable(),
  route: Route,
  recipe_snapshot: z.array(RecipeIngredientSnapshot),
});
export type OrderItemSpec = z.infer<typeof OrderItemSpec>;

export const NoticeKind = z.enum(["event_appended"]);
export type NoticeKind = z.infer<typeof NoticeKind>;

export const EventNotice = z.object({
  kind: NoticeKind,
  event_type: z.string(),
  aggregate_id: z.string(),
  ts: z.number().int(),
});
export type EventNotice = z.infer<typeof EventNotice>;
```

- [ ] **Step 3: schemas/auth.ts**

```ts
import { z } from "zod";
import { Role } from "./common";

export const LoginInput = z.object({ pin: z.string().min(6) });
export type LoginInput = z.infer<typeof LoginInput>;

export const TokenClaims = z.object({
  staff_id: z.number().int(),
  role: Role,
  iat: z.number().int(),
  exp: z.number().int(),
  jti: z.string(),
});
export type TokenClaims = z.infer<typeof TokenClaims>;

export const LoginOutput = z.object({
  token: z.string(),
  claims: TokenClaims,
});
export type LoginOutput = z.infer<typeof LoginOutput>;
```

- [ ] **Step 4: typecheck**

Run: `pnpm --filter @workspace/shared typecheck`
Expected: clean (other schemas not yet exporting; index.ts has missing imports — make those files empty stubs first OR comment out the future exports).

Actually — easier: comment out the `session/order/payment/catalog/client/ws` lines in `index.ts` for now; uncomment as each Task adds them. Or create stub files that just `export {}`.

- [ ] **Step 5: Commit**

```bash
git add packages/shared
git commit -m "feat(shared): zod schemas — error envelope + common types + auth"
```

---

## Task 11: zod schemas — session, order, payment, catalog

**Files:**
- Create: `packages/shared/src/schemas/{session,order,payment,catalog}.ts`

- [ ] **Step 1: session.ts**

```ts
import { z } from "zod";
import { SpotRef } from "./common";

export const SessionStatus = z.union([
  z.literal("Open"),
  z.literal("Closed"),
  z.object({ Merged: z.object({ into: z.string() }) }),
  z.literal("Split"),
]);
export type SessionStatus = z.infer<typeof SessionStatus>;

export const SessionState = z.object({
  session_id: z.string(),
  status: SessionStatus,
  spot: SpotRef,
  opened_by: z.number().int(),
  customer_label: z.string().nullable(),
  team: z.string().nullable(),
  order_ids: z.array(z.string()),
});
export type SessionState = z.infer<typeof SessionState>;

export const OpenSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  spot_id: z.number().int(),
  customer_label: z.string().nullable().optional(),
  team: z.string().nullable().optional(),
});
export type OpenSessionInput = z.infer<typeof OpenSessionInput>;

export const CloseSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  reason: z.string().nullable().optional(),
});
export type CloseSessionInput = z.infer<typeof CloseSessionInput>;

export const TransferSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  to_spot_id: z.number().int(),
});
export type TransferSessionInput = z.infer<typeof TransferSessionInput>;

export const MergeSessionsInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  into_session: z.string(),
  sources: z.array(z.string()),
});
export type MergeSessionsInput = z.infer<typeof MergeSessionsInput>;

export const SplitSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  new_sessions: z.array(z.string()),
});
export type SplitSessionInput = z.infer<typeof SplitSessionInput>;
```

- [ ] **Step 2: order.ts**

```ts
import { z } from "zod";
import { OrderItemSpec } from "./common";

export const OrderItem = z.object({
  spec: OrderItemSpec,
  cancelled: z.boolean(),
  returned_qty: z.number().int(),
});
export type OrderItem = z.infer<typeof OrderItem>;

export const OrderState = z.object({
  order_id: z.string(),
  session_id: z.string(),
  items: z.array(OrderItem),
});
export type OrderState = z.infer<typeof OrderState>;

export const RawOrderItem = z.object({
  product_id: z.number().int(),
  qty: z.number().int(),
  note: z.string().nullable().optional(),
});
export type RawOrderItem = z.infer<typeof RawOrderItem>;

export const PlaceOrderInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  session_id: z.string(),
  items: z.array(RawOrderItem),
});
export type PlaceOrderInput = z.infer<typeof PlaceOrderInput>;

export const CancelOrderItemInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  reason: z.string().nullable().optional(),
  is_self: z.boolean(),
  within_grace: z.boolean(),
});
export type CancelOrderItemInput = z.infer<typeof CancelOrderItemInput>;

export const ReturnOrderItemInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  qty: z.number().int(),
  reason: z.string().nullable().optional(),
});
export type ReturnOrderItemInput = z.infer<typeof ReturnOrderItemInput>;
```

- [ ] **Step 3: payment.ts**

```ts
import { z } from "zod";

export const PaymentState = z.object({
  session_id: z.string(),
  subtotal: z.number().int(),
  discount_pct: z.number().int(),
  vat_pct: z.number().int(),
  total: z.number().int(),
  method: z.string(),
});
export type PaymentState = z.infer<typeof PaymentState>;

export const TakePaymentInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  subtotal: z.number().int(),
  discount_pct: z.number().int(),
  vat_pct: z.number().int(),
  total: z.number().int(),
  method: z.string(),
});
export type TakePaymentInput = z.infer<typeof TakePaymentInput>;
```

- [ ] **Step 4: catalog.ts**

```ts
import { z } from "zod";
import { Role, SpotKind } from "./common";

export const Spot = z.object({
  id: z.number().int(),
  name: z.string(),
  kind: SpotKind,
  hourly_rate: z.number().int().nullable(),
  parent_id: z.number().int().nullable(),
  status: z.string(),
});
export type Spot = z.infer<typeof Spot>;

export const Product = z.object({
  id: z.number().int(),
  name: z.string(),
  price: z.number().int(),
  route: z.string(),    // server returns string; client may parse via Route enum
  kind: z.string(),
});
export type Product = z.infer<typeof Product>;

export const StaffOut = z.object({
  id: z.number().int(),
  name: z.string(),
  role: z.string(),     // string — server uses Role enum but TS keeps loose
  team: z.string().nullable(),
});
export type StaffOut = z.infer<typeof StaffOut>;
```

- [ ] **Step 5: Re-enable index.ts exports + typecheck + commit**

Uncomment session/order/payment/catalog lines in `index.ts`. Run `pnpm --filter @workspace/shared typecheck`. Commit:

```bash
git add packages/shared
git commit -m "feat(shared): zod schemas for session, order, payment, catalog"
```

---

## Task 12: API client (typed fetch)

**Files:** Create `packages/shared/src/client.ts`

- [ ] **Step 1: Implement**

```ts
import { z } from "zod";
import { ApiError, AppErrorEnvelope } from "./error";

export interface ApiClientOptions {
  /** Base URL like http://localhost:7878 (no trailing slash). */
  baseUrl: string;
  /** Returns the current bearer token, or null if not authenticated. */
  getToken: () => string | null;
}

export class ApiClient {
  constructor(private opts: ApiClientOptions) {}

  async request<O extends z.ZodTypeAny>(
    method: "GET" | "POST",
    path: string,
    output: O,
    body?: unknown,
  ): Promise<z.infer<O>> {
    const url = `${this.opts.baseUrl}${path}`;
    const headers: Record<string, string> = {};
    const token = this.opts.getToken();
    if (token) headers["Authorization"] = `Bearer ${token}`;
    if (body !== undefined) headers["Content-Type"] = "application/json";

    const resp = await fetch(url, {
      method, headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });

    if (!resp.ok) {
      let envelope: AppErrorEnvelope;
      try {
        const raw = await resp.json();
        envelope = AppErrorEnvelope.parse(raw);
      } catch {
        envelope = { code: "internal", message: resp.statusText };
      }
      throw new ApiError(resp.status, envelope);
    }

    if (resp.status === 204) return output.parse(undefined);

    const json = await resp.json();
    return output.parse(json);
  }

  get<O extends z.ZodTypeAny>(path: string, output: O) {
    return this.request("GET", path, output);
  }

  post<O extends z.ZodTypeAny>(path: string, output: O, body?: unknown) {
    return this.request("POST", path, output, body);
  }
}
```

- [ ] **Step 2: Commit**

```bash
git add packages/shared
git commit -m "feat(shared): ApiClient — typed fetch with zod parsing + ApiError throw"
```

---

## Task 13: WS hook with first-message handshake

**Files:** Create `packages/shared/src/ws.ts`

- [ ] **Step 1: Implement**

```ts
import { EventNotice } from "./schemas/common";

export interface EventStreamOptions {
  /** Base URL like ws://localhost:7878 (no trailing slash). */
  baseUrl: string;
  getToken: () => string | null;
  onNotice: (notice: import("./schemas/common").EventNotice) => void;
  onError?: (err: Error) => void;
  onOpen?: () => void;
  onClose?: () => void;
}

/// Connects to /ws, does the {type:hello, token} handshake, then dispatches
/// EventNotice payloads to onNotice. Returns a teardown function.
export function createEventStream(opts: EventStreamOptions): () => void {
  const url = `${opts.baseUrl}/ws`;
  const ws = new WebSocket(url);
  let helloSent = false;

  ws.addEventListener("open", () => {
    const token = opts.getToken();
    if (!token) {
      ws.close(4001, "no token");
      return;
    }
    ws.send(JSON.stringify({ type: "hello", token }));
    helloSent = true;
  });

  ws.addEventListener("message", (ev) => {
    try {
      const data = JSON.parse(typeof ev.data === "string" ? ev.data : "");
      if (data && data.type === "hello_ok") {
        opts.onOpen?.();
        return;
      }
      if (data && data.type === "error") {
        opts.onError?.(new Error(`ws error: ${data.code}`));
        return;
      }
      // Otherwise expect an EventNotice
      const notice = EventNotice.parse(data);
      opts.onNotice(notice);
    } catch (e) {
      opts.onError?.(e as Error);
    }
  });

  ws.addEventListener("close", () => opts.onClose?.());
  ws.addEventListener("error", () => opts.onError?.(new Error("ws transport error")));

  return () => ws.close();
}
```

- [ ] **Step 2: Typecheck + commit**

```bash
pnpm --filter @workspace/shared typecheck
git add packages/shared
git commit -m "feat(shared): createEventStream — WS with first-message handshake"
```

---

## Task 14: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — all pass (~178)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `pnpm --filter @workspace/shared typecheck` clean
- [ ] `pnpm --filter @tableorder/cashier typecheck` clean (still passes — cashier doesn't yet import @workspace/shared)

---

## Done

End state:

**Backend:**
- PIN min length 6 enforced in `hash_pin`
- `OverrideRequired` returns HTTP 403 (was 401)
- `TokenClaims.jti: String` (UUID v4 generated at login)
- `master.token_denylist` table + `AuthService::revoke`
- `POST /auth/logout` endpoint
- WS auth via first-message handshake (browser-compatible)
- `/auth/login` rate-limited to 10/IP/min via `tower_governor` (returns 429)

**Shared TS client:**
- `packages/shared` exports zod schemas mirroring every server-side type
- `ApiClient` typed-fetch wrapper that throws `ApiError` on non-2xx
- `createEventStream` browser WS hook with handshake

**Test count:** ~180 (Rust) + new TS package typecheck

What this unblocks:
- **Plan E1c** writes `apps/web` (tablet PWA) and rewires `apps/cashier`'s React app — both consume `@workspace/shared`. UI work; no new backend.
- Idle lock UI (frontend) lands in E1c.
- External integrations (curl, scripts) still work against HTTP directly without the TS client.

Plan F still owns:
- EOD pipeline + reports
- Print stubs
- Idempotency cleanup at EOD
- Token denylist pruning (deletes rows where `expires_at < now`)
