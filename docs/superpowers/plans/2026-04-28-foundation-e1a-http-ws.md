# Foundation Plan E1a — HTTP + WS as the Sole Command Surface

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make axum HTTP+WS the **only** API surface. Delete the Tauri command shims (`commands/`) — the cashier React UI will talk to localhost over HTTP+WS like any tablet does. End state: every client (cashier window, tablet PWA, future external integrations) hits the same routes; one API to maintain.

**Architecture:** axum 0.7 server bound to `0.0.0.0:7878` (configurable), spawned inside Tauri's main process on the existing Tokio runtime. Sync `CommandService` calls wrapped in `spawn_blocking`. Auth via **`Authorization: Bearer <token>` header only** (no cookie — defeats CSRF from same-LAN browsers). Rate-limited login. WS broadcast on `WriteOutcome::Inserted`. AppError mapped to JSON status responses.

Tauri stays as the OS shell: native window, OS keychain access (Rust internal — never exposed), app lifecycle. No Tauri *commands*.

**Tech Stack:** Adds `axum`, `tokio` (full), `tower`, `tower-http` (with `limit` for rate-limiting), `axum-extra` (kept for header utilities; cookie support unused).

**Spec:** `docs/superpowers/specs/2026-04-27-foundation-design.md` — sections "Architecture", "API Surface", "WS events".

**Builds on:** A through E0.5. Per Plan E1+F prep notes, addresses items 7, 8, 9. Adds explicit security hardening based on shared-wifi threat analysis.

**Out of scope:**
- `packages/shared` TS client → Plan E1b
- `apps/web` tablet PWA → Plan E1b
- Cashier React app — its build pipeline still works; what it CALLS now becomes HTTP, not Tauri IPC. Re-wiring the React side belongs to E1b.
- `idle_lock_minutes` UI enforcement → Plan E1b
- TLS / HTTPS → out of scope (deferred to security-hardening plan; mitigated by mandatory staff-wifi separation)
- Failed-login lockout table → out of scope (mitigated by rate-limiting; revisit after E1b)
- Token sliding TTL / refresh endpoint → out of scope
- EOD trigger HTTP endpoint → Plan F

**Required deployment posture (documented in README):** staff network MUST be on a separate SSID/VLAN from customer wifi. Without this, no further mitigation in this plan keeps the API safe.

---

## File Structure

```
apps/cashier/src-tauri/src/
  http/
    mod.rs                              # NEW
    server.rs                           # NEW — axum bind + serve loop
    auth_layer.rs                       # NEW — Bearer-header extractor
    error_layer.rs                      # NEW — IntoResponse for AppError
    broadcast.rs                        # NEW — broadcast channel + EventNotice
    spot_helper.rs                      # NEW — build_spot_ref shared by session routes
    routes/
      mod.rs                            # NEW
      auth.rs                           # NEW — POST /auth/login (rate-limited), POST /auth/logout, GET /auth/me
      catalog.rs                        # NEW — GET /staff, GET /spots, GET /products
      session.rs                        # NEW — POST /sessions, /sessions/:id/close, transfer, merge, split, GET /sessions/active, GET /sessions/:id
      order.rs                          # NEW — POST /orders, /orders/:id/items/:idx/cancel, return
      payment.rs                        # NEW — POST /sessions/:id/payment
      ws.rs                             # NEW — GET /ws (subscribes to event notices)
  app_state.rs                          # MODIFIED — Settings cache + broadcast tx; AppState becomes pure axum state
  services/command_service.rs           # MODIFIED — execute fires broadcast on Inserted
  lib.rs                                # MODIFIED — spawn axum task on startup; no command registration
  commands/                             # DELETED — all 5 files removed; logic inlined into HTTP routes
apps/cashier/README.md                  # MODIFIED — security posture section added
tests/
  http_integration.rs                   # NEW — reqwest-driven end-to-end smoke + auth + WS + rate-limit
  commands_integration.rs               # MODIFIED — calls CommandService directly (was via Tauri); minor sweep
  hardening_integration.rs              # unchanged (already calls CommandService directly)
  override_integration.rs               # unchanged
  remaining_commands_integration.rs     # unchanged
  validation_integration.rs             # unchanged
  aggregate_store_integration.rs        # unchanged
  projection_integration.rs             # unchanged
```

Decisions:
- **No Tauri commands.** Single source of truth = HTTP route handlers. Cashier React app fetches `http://localhost:7878/...` exactly like the tablet does.
- **Bearer-header auth only.** No cookies → no CSRF surface for browsers on the same LAN. Token lives in localStorage on the client side.
- **Rate-limit `/auth/login`** at 10 attempts/IP/min via `tower-http::limit::RateLimitLayer` (or `tower::limit::ConcurrencyLimitLayer` if `RateLimitLayer` doesn't fit; pick what compiles cleanest).
- **CORS `allow_origin(Any)`** is acceptable BECAUSE there's no cookie auth — a browser visiting evil.com can't ride the cashier's existing session.
- **`AppState`** stays `Send + Sync + 'static`; passed to axum via `with_state(Arc<AppState>)`. Tauri no longer manages it.

---

## Task 1: Add deps

**Files:** Modify `apps/cashier/src-tauri/Cargo.toml`

- [ ] **Step 1: Add to `[dependencies]`**

```toml
axum = { version = "0.7", features = ["ws", "macros"] }
tokio = { version = "1", features = ["full"] }
tower = { version = "0.5", features = ["limit"] }
tower-http = { version = "0.6", features = ["cors", "trace", "limit"] }
axum-extra = "0.9"
```

(`axum-extra` retained for future header utilities; cookie subfeature dropped.)

- [ ] **Step 2: Verify**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: deps resolve.

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat(cashier): add axum + tokio + tower deps for HTTP layer"
```

---

## Task 2: Settings cache + AppState extensions + broadcast types

**Files:**
- Modify: `apps/cashier/src-tauri/src/app_state.rs`
- Create: `apps/cashier/src-tauri/src/http/mod.rs`
- Create: `apps/cashier/src-tauri/src/http/broadcast.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Settings struct**

In `app_state.rs`:
```rust
use chrono::FixedOffset;

#[derive(Debug, Clone)]
pub struct Settings {
    pub business_day_cutoff_hour: u32,
    pub business_day_tz: FixedOffset,
    pub discount_threshold_pct: u32,
    pub cancel_grace_minutes: u32,
    pub idle_lock_minutes: u32,
    pub http_port: u16,
}

impl Settings {
    pub fn load(master: &crate::store::master::Master) -> crate::error::AppResult<Self> {
        fn req<T: std::str::FromStr>(master: &crate::store::master::Master, key: &str)
            -> crate::error::AppResult<T>
        where <T as std::str::FromStr>::Err: std::fmt::Display
        {
            let s = master.get_setting(key)?
                .ok_or_else(|| crate::error::AppError::Config(format!("setting missing: {key}")))?;
            s.parse::<T>().map_err(|e| crate::error::AppError::Config(format!("setting {key} parse: {e}")))
        }
        let cutoff: u32 = req(master, "business_day_cutoff_hour")?;
        let tz_seconds: i32 = req(master, "business_day_tz_offset_seconds")?;
        let tz = FixedOffset::east_opt(tz_seconds)
            .ok_or_else(|| crate::error::AppError::Config(format!("bad tz offset: {tz_seconds}")))?;
        let discount_threshold_pct: u32 = req(master, "discount_threshold_pct")?;
        let cancel_grace_minutes: u32 = req(master, "cancel_grace_minutes")?;
        let idle_lock_minutes: u32 = req(master, "idle_lock_minutes")?;
        let http_port: u16 = master.get_setting("http_port")?
            .map(|s| s.parse().unwrap_or(7878))
            .unwrap_or(7878);
        Ok(Self {
            business_day_cutoff_hour: cutoff, business_day_tz: tz,
            discount_threshold_pct, cancel_grace_minutes,
            idle_lock_minutes, http_port,
        })
    }
}
```

- [ ] **Step 2: Extend AppState**

```rust
use std::sync::Arc;

pub struct AppState {
    pub kek: Arc<Kek>,
    pub master: Arc<Mutex<Master>>,
    pub events: Arc<EventStore>,
    pub clock: Arc<dyn Clock>,
    pub auth: AuthService,
    pub commands: CommandService,
    pub store: Arc<AggregateStore>,
    pub settings: Arc<Settings>,                           // NEW
    pub broadcast_tx: tokio::sync::broadcast::Sender<crate::http::broadcast::EventNotice>,  // NEW
}
```

- [ ] **Step 3: Broadcast types**

`http/mod.rs`:
```rust
pub mod broadcast;
```

`http/broadcast.rs`:
```rust
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct EventNotice {
    pub kind: &'static str,
    pub event_type: String,
    pub aggregate_id: String,
    pub ts: i64,
}

impl EventNotice {
    pub fn appended(event_type: impl Into<String>, aggregate_id: impl Into<String>, ts: i64) -> Self {
        Self { kind: "event.appended", event_type: event_type.into(), aggregate_id: aggregate_id.into(), ts }
    }
}
```

- [ ] **Step 4: Wire `pub mod http;` into lib.rs**

- [ ] **Step 5: Verify**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: compiles. Tests still 154 — settings/broadcast not yet exercised.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "feat(cashier): Settings + broadcast channel on AppState"
```

---

## Task 3: `CommandService` fires broadcast on Inserted

**Files:** Modify `apps/cashier/src-tauri/src/services/command_service.rs`

- [ ] **Step 1: Add broadcast field**

```rust
pub struct CommandService {
    /* existing fields */
    pub broadcast_tx: tokio::sync::broadcast::Sender<crate::http::broadcast::EventNotice>,
}
```

- [ ] **Step 2: Fire on Inserted**

In `execute`, AFTER `apply` runs successfully and BEFORE `put_idempotency`:
```rust
let _ = self.broadcast_tx.send(crate::http::broadcast::EventNotice::appended(
    event.event_type().as_str(),
    aggregate_id,
    self.clock.now().timestamp_millis(),
));
```

`send` returns `Result<usize, SendError<T>>`; failure means no live receivers — ignored.

- [ ] **Step 3: Update test rigs**

The 7 integration test files build `CommandService { ... }` literals. Add `broadcast_tx`. Add a helper to `tests/common/mod.rs`:
```rust
pub fn dummy_broadcast() -> tokio::sync::broadcast::Sender<cashier_lib::http::broadcast::EventNotice> {
    let (tx, _rx) = tokio::sync::broadcast::channel(16);
    tx
}
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: 154 still pass.

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(cashier): CommandService broadcasts EventNotice on Inserted writes"
```

---

## Task 4: AppError → IntoResponse

**Files:** Create `apps/cashier/src-tauri/src/http/error_layer.rs`

- [ ] **Step 1: Implement**

```rust
use crate::error::AppError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

pub struct AppErrorResponse(pub AppError);

impl From<AppError> for AppErrorResponse {
    fn from(e: AppError) -> Self { Self(e) }
}

impl IntoResponse for AppErrorResponse {
    fn into_response(self) -> Response {
        let status = match &self.0 {
            AppError::Validation(_)              => StatusCode::BAD_REQUEST,
            AppError::Conflict(_)                => StatusCode::CONFLICT,
            AppError::NotFound                   => StatusCode::NOT_FOUND,
            AppError::Unauthorized               => StatusCode::UNAUTHORIZED,
            AppError::OverrideRequired(_)        => StatusCode::UNAUTHORIZED,
            AppError::Db(_)
            | AppError::Crypto(_)
            | AppError::Keychain(_)
            | AppError::Io(_)
            | AppError::Config(_)
            | AppError::Internal(_)              => StatusCode::INTERNAL_SERVER_ERROR,
        };
        let body = serde_json::to_value(&self.0)
            .unwrap_or_else(|_| json!({"code":"internal"}));
        (status, Json(body)).into_response()
    }
}
```

- [ ] **Step 2: Wire**

`http/mod.rs`:
```rust
pub mod broadcast;
pub mod error_layer;
```

- [ ] **Step 3: 4 unit tests** (mapped status + body shape, per Plan E1a v1 Task 4 — same shape).

- [ ] **Step 4: Run + commit**

```bash
cargo test --lib http::error_layer
git add -u
git commit -m "feat(cashier): AppError → HTTP status mapping with JSON envelope"
```

---

## Task 5: Bearer-header AuthCtx extractor

**Files:** Create `apps/cashier/src-tauri/src/http/auth_layer.rs`

- [ ] **Step 1: Implement (header-only, no cookie)**

```rust
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::error::AppError;
use crate::http::error_layer::AppErrorResponse;
use axum::async_trait;
use axum::extract::FromRequestParts;
use axum::http::request::Parts;
use std::sync::Arc;

/// Extracted from incoming requests via the `Authorization: Bearer <token>`
/// header. No cookie fallback by design — cookies enable CSRF when the
/// cashier API is reachable from a browser on the same LAN.
pub struct AuthCtx(pub TokenClaims);

#[async_trait]
impl FromRequestParts<Arc<AppState>> for AuthCtx {
    type Rejection = AppErrorResponse;

    async fn from_request_parts(parts: &mut Parts, state: &Arc<AppState>)
        -> Result<Self, Self::Rejection>
    {
        let token = parts.headers.get("authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|s| s.strip_prefix("Bearer "))
            .ok_or_else(|| AppErrorResponse(AppError::Unauthorized))?;
        let claims = state.auth.verify(token).map_err(AppErrorResponse)?;
        Ok(AuthCtx(claims))
    }
}
```

- [ ] **Step 2: Tests + commit**

3 unit tests on the parsing logic (header present, header missing, malformed).

```bash
git add -u
git commit -m "feat(cashier): AuthCtx Bearer-header extractor (no cookie — CSRF-safe)"
```

---

## Task 6: Spot helper + remaining shared utilities

**Files:** Create `apps/cashier/src-tauri/src/http/spot_helper.rs`

- [ ] **Step 1: `build_spot_ref`**

The session routes need the same Spot-to-SpotRef logic that the deleted Tauri commands used. Centralize here:

```rust
use crate::app_state::AppState;
use crate::domain::spot::SpotRef;
use crate::error::{AppError, AppResult};
use crate::store::master::{Spot, SpotKind};
use std::sync::Arc;

/// Resolve a Master Spot row into a SpotRef (snapshot for event payload).
/// For tables with parent_id, looks up the parent room to populate room_name.
pub fn build_spot_ref(state: &Arc<AppState>, spot: Spot) -> AppResult<SpotRef> {
    Ok(match spot.kind {
        SpotKind::Room => SpotRef::Room {
            id: spot.id, name: spot.name,
            hourly_rate: spot.hourly_rate
                .ok_or_else(|| AppError::Validation("room missing rate".into()))?,
        },
        SpotKind::Table => {
            let (room_id, room_name) = if let Some(pid) = spot.parent_id {
                match state.master.lock().unwrap().get_spot(pid)? {
                    Some(p) => (Some(p.id), Some(p.name)),
                    None => (None, None),
                }
            } else { (None, None) };
            SpotRef::Table { id: spot.id, name: spot.name, room_id, room_name }
        }
    })
}
```

`http/mod.rs`:
```rust
pub mod spot_helper;
```

- [ ] **Step 2: Commit**

```bash
git add -u
git commit -m "feat(cashier): build_spot_ref shared helper for session routes"
```

---

## Task 7: `/auth` routes

**Files:** Create `apps/cashier/src-tauri/src/http/routes/{mod.rs, auth.rs}`

- [ ] **Step 1: Implement**

`routes/mod.rs`:
```rust
pub mod auth;
```

`routes/auth.rs`:
```rust
use crate::app_state::AppState;
use crate::auth::token::TokenClaims;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Deserialize)]
pub struct LoginInput { pub pin: String }

#[derive(Serialize)]
pub struct LoginOutput { pub token: String, pub claims: TokenClaims }

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/me", get(me))
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(input): Json<LoginInput>,
) -> Result<Json<LoginOutput>, AppErrorResponse> {
    let auth = state.auth.clone();
    let result = tokio::task::spawn_blocking(move || auth.login(&input.pin)).await
        .map_err(|e| AppErrorResponse(crate::error::AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(LoginOutput { token: result.0, claims: result.1 }))
}

async fn me(AuthCtx(claims): AuthCtx) -> Json<TokenClaims> {
    Json(claims)
}
```

`/auth/logout` is a no-op when there's no server-side session list — the client just discards the token. Skip the route. (If revocation lands later: add a denylist + `/auth/logout` route.)

- [ ] **Step 2: Commit**

```bash
git add -u
git commit -m "feat(cashier): POST /auth/login + GET /auth/me HTTP routes"
```

---

## Task 8: catalog routes

**Files:** Create `apps/cashier/src-tauri/src/http/routes/catalog.rs`

- [ ] **Step 1: Implement**

```rust
use crate::app_state::AppState;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use crate::store::master::{Product, Spot, Staff};
use axum::extract::State;
use axum::routing::get;
use axum::{Json, Router};
use serde::Serialize;
use std::sync::Arc;

#[derive(Serialize)]
pub struct StaffOut {
    pub id: i64, pub name: String, pub role: String, pub team: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/staff", get(list_staff))
        .route("/spots", get(list_spots))
        .route("/products", get(list_products))
}

async fn list_staff(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<StaffOut>>, AppErrorResponse> {
    let master = state.master.clone();
    let staff = tokio::task::spawn_blocking(move || master.lock().unwrap().list_staff()).await
        .map_err(|e| AppErrorResponse(crate::error::AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(staff.into_iter().map(|s| StaffOut {
        id: s.id, name: s.name, role: s.role.as_str().into(), team: s.team,
    }).collect()))
}

async fn list_spots(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<Spot>>, AppErrorResponse> {
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_spots()).await
        .map_err(|e| AppErrorResponse(crate::error::AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

async fn list_products(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<Vec<Product>>, AppErrorResponse> {
    let master = state.master.clone();
    let r = tokio::task::spawn_blocking(move || master.lock().unwrap().list_products()).await
        .map_err(|e| AppErrorResponse(crate::error::AppError::Internal(format!("join: {e}"))))?
        .map_err(AppErrorResponse)?;
    Ok(Json(r))
}
```

`routes/mod.rs` add `pub mod catalog;`.

- [ ] **Step 2: Commit**

```bash
git add -u
git commit -m "feat(cashier): /staff /spots /products GET routes"
```

---

## Task 9: session/order/payment routes

**Files:** Create `apps/cashier/src-tauri/src/http/routes/{session,order,payment}.rs`

The bodies are inlined from the deleted Tauri commands (since `commands/` will be removed in Task 11). Each handler:
1. Extracts `AuthCtx`
2. Deserializes input (define input structs in the route file)
3. Looks up snapshots in master (via `spot_helper::build_spot_ref` for sessions; via `master.get_product`/`get_recipe` for orders)
4. Builds `DomainEvent`
5. Calls `state.commands.execute(...)` inside `spawn_blocking`
6. Returns projection JSON

- [ ] **Step 1: session.rs**

```rust
use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::DomainEvent;
use crate::domain::session::SessionState;
use crate::error::AppError;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use crate::http::spot_helper::build_spot_ref;
use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct OpenSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub spot_id: i64,
    pub customer_label: Option<String>,
    pub team: Option<String>,
}

#[derive(Deserialize)]
pub struct CloseSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub reason: Option<String>,
}

#[derive(Deserialize)]
pub struct TransferSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub to_spot_id: i64,
}

#[derive(Deserialize)]
pub struct MergeSessionsInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub into_session: String,
    pub sources: Vec<String>,
}

#[derive(Deserialize)]
pub struct SplitSessionInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub new_sessions: Vec<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/sessions", post(open_session))
        .route("/sessions/active", get(list_active))
        .route("/sessions/:id", get(get_session))
        .route("/sessions/:id/close", post(close_session))
        .route("/sessions/:id/transfer", post(transfer_session))
        .route("/sessions/merge", post(merge_sessions))
        .route("/sessions/:id/split", post(split_session))
}

async fn open_session(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<OpenSessionInput>,
) -> Result<Json<SessionState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<SessionState, AppError> {
        let spot = s.master.lock().unwrap()
            .get_spot(input.spot_id)?
            .ok_or(AppError::NotFound)?;
        let spot_ref = build_spot_ref(&s, spot)?;
        let session_id = Uuid::new_v4().to_string();
        let event = DomainEvent::SessionOpened {
            spot: spot_ref,
            opened_by: claims.staff_id,
            customer_label: input.customer_label,
            team: input.team,
        };
        let (proj, _) = s.commands.execute(
            &claims, Action::OpenSession, PolicyCtx::default(),
            &input.idempotency_key, "open_session", &session_id,
            event, input.override_pin.as_deref(),
            |c| c.load_session(&session_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    }).await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

// close_session, transfer_session, merge_sessions, split_session,
// list_active, get_session — same pattern, mechanical.
```

Implement all 7 handlers. The bodies are taken verbatim from the deleted Tauri commands (with the `state.auth.verify(&input.token)` step replaced by the `AuthCtx` extractor).

- [ ] **Step 2: order.rs**

```rust
use crate::acl::{policy::PolicyCtx, Action};
use crate::app_state::AppState;
use crate::domain::event::{DomainEvent, OrderItemSpec, RecipeIngredientSnapshot, Route};
use crate::domain::order::OrderState;
use crate::error::AppError;
use crate::http::auth_layer::AuthCtx;
use crate::http::error_layer::AppErrorResponse;
use axum::extract::{Path, State};
use axum::routing::post;
use axum::{Json, Router};
use serde::Deserialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Deserialize)]
pub struct RawOrderItem {
    pub product_id: i64,
    pub qty: i64,
    pub note: Option<String>,
}

#[derive(Deserialize)]
pub struct PlaceOrderInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub session_id: String,
    pub items: Vec<RawOrderItem>,
}

#[derive(Deserialize)]
pub struct CancelOrderItemInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub reason: Option<String>,
    pub is_self: bool,
    pub within_grace: bool,
}

#[derive(Deserialize)]
pub struct ReturnOrderItemInput {
    pub idempotency_key: String,
    pub override_pin: Option<String>,
    pub qty: i64,
    pub reason: Option<String>,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/orders", post(place_order))
        .route("/orders/:order_id/items/:idx/cancel", post(cancel_item))
        .route("/orders/:order_id/items/:idx/return", post(return_item))
}

fn parse_route(s: &str) -> Result<Route, AppError> {
    match s { "kitchen" => Ok(Route::Kitchen), "bar" => Ok(Route::Bar), "none" => Ok(Route::None),
              other => Err(AppError::Validation(format!("bad route: {other}"))) }
}

async fn place_order(
    State(state): State<Arc<AppState>>,
    AuthCtx(claims): AuthCtx,
    Json(input): Json<PlaceOrderInput>,
) -> Result<Json<OrderState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<OrderState, AppError> {
        let mut spec_items = Vec::with_capacity(input.items.len());
        {
            let m = s.master.lock().unwrap();
            for raw in input.items {
                let p = m.get_product(raw.product_id)?
                    .ok_or_else(|| AppError::Validation(format!("product {} not found", raw.product_id)))?;
                let recipe = m.get_recipe(p.id)?;
                let recipe_snapshot = recipe.into_iter().map(|ing| RecipeIngredientSnapshot {
                    ingredient_id: ing.ingredient_id,
                    ingredient_name: ing.ingredient_name,
                    qty: ing.qty, unit: ing.unit,
                }).collect();
                spec_items.push(OrderItemSpec {
                    product_id: p.id, product_name: p.name,
                    qty: raw.qty, unit_price: p.price,
                    note: raw.note,
                    route: parse_route(&p.route)?,
                    recipe_snapshot,
                });
            }
        }
        let order_id = Uuid::new_v4().to_string();
        let event = DomainEvent::OrderPlaced {
            session_id: input.session_id.clone(),
            order_id: order_id.clone(),
            items: spec_items,
        };
        let (proj, _) = s.commands.execute(
            &claims, Action::PlaceOrder, PolicyCtx::default(),
            &input.idempotency_key, "place_order", &order_id,
            event, input.override_pin.as_deref(),
            |c| c.load_order(&order_id)?.ok_or(AppError::NotFound),
        )?;
        Ok(proj)
    }).await
    .map_err(|e| AppErrorResponse(AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}

// cancel_item, return_item — Path((order_id, idx)) extracted; bodies follow same pattern.
```

- [ ] **Step 3: payment.rs**

```rust
// take_payment with Path(session_id) — body inlined from deleted Tauri command.
// Action variant computed from discount_pct vs settings.discount_threshold_pct.
```

- [ ] **Step 4: Wire**

`routes/mod.rs`:
```rust
pub mod auth;
pub mod catalog;
pub mod order;
pub mod payment;
pub mod session;
pub mod ws;
```

(`ws` is Task 10.)

- [ ] **Step 5: Commit**

```bash
git add -u
git commit -m "feat(cashier): session/order/payment HTTP routes (inlined from former Tauri cmds)"
```

---

## Task 10: WS broadcast endpoint

**Files:** Create `apps/cashier/src-tauri/src/http/routes/ws.rs`

- [ ] **Step 1: Implement**

```rust
use crate::app_state::AppState;
use crate::http::auth_layer::AuthCtx;
use crate::http::broadcast::EventNotice;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route("/ws", get(ws_handler))
}

async fn ws_handler(
    State(state): State<Arc<AppState>>,
    _auth: AuthCtx,
    ws: WebSocketUpgrade,
) -> Response {
    let rx = state.broadcast_tx.subscribe();
    ws.on_upgrade(move |socket| client_loop(socket, rx))
}

async fn client_loop(
    mut socket: WebSocket,
    mut rx: tokio::sync::broadcast::Receiver<EventNotice>,
) {
    while let Ok(notice) = rx.recv().await {
        let json = match serde_json::to_string(&notice) {
            Ok(s) => s,
            Err(e) => { tracing::warn!(?e, "ws serialize"); continue; }
        };
        if socket.send(Message::Text(json)).await.is_err() { break; }
    }
}
```

- [ ] **Step 2: Commit**

```bash
git add -u
git commit -m "feat(cashier): GET /ws WebSocket subscribes to event notices"
```

---

## Task 11: Delete Tauri commands; spawn axum from setup

**Files:**
- Delete: `apps/cashier/src-tauri/src/commands/` (entire directory)
- Modify: `apps/cashier/src-tauri/src/lib.rs`
- Create: `apps/cashier/src-tauri/src/http/server.rs`
- Modify: `apps/cashier/src-tauri/tests/commands_integration.rs` (remove the 4 tests that depended on Tauri command bodies — wait, they actually call `cs.execute` directly, so they survive; verify and only remove imports of `commands::*` if any)

- [ ] **Step 1: Build router + serve**

```rust
// http/server.rs
use crate::app_state::AppState;
use crate::error::{AppError, AppResult};
use axum::Router;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceBuilder;
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tower_http::limit::RequestBodyLimitLayer;

pub fn build_router(state: Arc<AppState>) -> Router {
    // Login is rate-limited via a per-route layer; mounted as its own sub-router.
    let login_only = Router::new()
        .merge(crate::http::routes::auth::router())
        .layer(
            ServiceBuilder::new()
                .layer(tower::limit::RateLimitLayer::new(10, Duration::from_secs(60)))
        );

    Router::new()
        .merge(login_only)
        .merge(crate::http::routes::catalog::router())
        .merge(crate::http::routes::session::router())
        .merge(crate::http::routes::order::router())
        .merge(crate::http::routes::payment::router())
        .merge(crate::http::routes::ws::router())
        .with_state(state)
        .layer(CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any))
        .layer(RequestBodyLimitLayer::new(64 * 1024))     // 64KB body cap
        .layer(TraceLayer::new_for_http())
}

pub async fn serve(state: Arc<AppState>) -> AppResult<()> {
    let port = state.settings.http_port;
    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let router = build_router(state);
    tracing::info!(%addr, "axum http server listening");
    let listener = tokio::net::TcpListener::bind(addr).await
        .map_err(AppError::Io)?;
    axum::serve(listener, router).await
        .map_err(|e| AppError::Internal(format!("axum serve: {e}")))?;
    Ok(())
}
```

`http/mod.rs`:
```rust
pub mod auth_layer;
pub mod broadcast;
pub mod error_layer;
pub mod routes;
pub mod server;
pub mod spot_helper;
```

Note: `RateLimitLayer` from `tower::limit` works without a service-name arg; if its API doesn't fit, swap to `tower-http::limit::RateLimitLayer` or `governor` crate.

- [ ] **Step 2: Delete `commands/`**

```bash
git rm -r apps/cashier/src-tauri/src/commands
```

- [ ] **Step 3: Update lib.rs**

```rust
pub mod app_state;
pub mod auth;
pub mod acl;
pub mod bootstrap;
pub mod business_day;
pub mod crypto;
pub mod domain;
pub mod error;
pub mod http;                                              // NEW
pub mod keychain;
pub mod services;
pub mod store;
pub mod time;
// pub mod commands;                                        // DELETED

pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // ... existing master/events/kek/auth/store construction ...
            // ... existing aggregate_store::warm_up ...

            let settings = Arc::new(app_state::Settings::load(&master.lock().unwrap())?);
            let (broadcast_tx, _) = tokio::sync::broadcast::channel(256);

            let commands = services::command_service::CommandService {
                master: master.clone(),
                events: events.clone(),
                event_service,
                clock: clock.clone(),
                auth: Arc::new(auth.clone()),
                idem_lock: Arc::new(services::locking::KeyMutex::new()),
                agg_lock: Arc::new(services::locking::KeyMutex::new()),
                store: store.clone(),
                broadcast_tx: broadcast_tx.clone(),
            };

            let app_state = Arc::new(app_state::AppState {
                kek, master, events, clock, auth, commands, store,
                settings, broadcast_tx,
            });

            // Spawn HTTP server on the existing Tokio runtime.
            let http_state = app_state.clone();
            tauri::async_runtime::spawn(async move {
                if let Err(e) = http::server::serve(http_state).await {
                    tracing::error!(?e, "http server exited with error");
                }
            });

            // Tauri no longer manages AppState — UI talks HTTP.
            Ok(())
        })
        // .invoke_handler(...) — DELETED; no Tauri commands.
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

(Use `tauri::async_runtime::spawn` to inherit Tauri's runtime; `tokio::spawn` works only inside a Tokio context.)

- [ ] **Step 4: Sweep tests**

`grep -l 'crate::commands\|cashier_lib::commands' tests/` — find any references. The integration tests already call `CommandService::execute` directly, so most are unaffected. The `commands_integration.rs` file may import a few types from the deleted modules (e.g., `LoginInput` type) — replace with the new types defined in HTTP route files (e.g., `cashier_lib::http::routes::auth::LoginInput`).

If a test was specifically asserting Tauri command behavior (rare), delete it — HTTP integration in T12 covers the same path.

- [ ] **Step 5: cargo check + cargo test**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: prior tests pass minus any deleted; ~150 still pass.

- [ ] **Step 6: Commit**

```bash
git add -u
git commit -m "feat(cashier): drop Tauri commands; HTTP+WS is the sole API surface

axum spawned in Tauri setup() on the inherited Tokio runtime. Tauri
stays as the OS shell (window, keychain access via Rust internals).
Cashier React UI will talk to localhost over HTTP/WS like any tablet.

Single API surface eliminates 16 thin command shims and the drift
risk between Tauri/HTTP handlers."
```

---

## Task 12: HTTP integration test

**Files:** Create `apps/cashier/src-tauri/tests/http_integration.rs`

- [ ] **Step 1: Test rig**

Boot `axum::serve` on `127.0.0.1:0` (ephemeral), record port, drive with `reqwest`. The rig builds a full `AppState` with in-memory Master + EventStore + AggregateStore.

- [ ] **Step 2: Tests**

- `http_login_returns_token` — POST /auth/login → 200 + `{token, claims}`. No `Set-Cookie` (cookie auth dropped).
- `http_unauthenticated_request_returns_401` — GET /spots without `Authorization` header → 401, body `{code: "unauthorized"}`.
- `http_invalid_token_returns_401` — GET /spots with `Authorization: Bearer junk` → 401.
- `http_full_session_lifecycle` — login → POST /sessions (open) → POST /orders (place) → POST /sessions/:id/payment (pay) → POST /sessions/:id/close. Each returns 200 with the expected projection.
- `http_ws_receives_event_notice_on_write` — connect to `/ws` (with token in `Sec-WebSocket-Protocol` or via initial Authorization header — verify which axum supports cleanly). Then HTTP POST /sessions; expect WS message `{kind: "event.appended", event_type: "SessionOpened", aggregate_id: ...}`.
- `http_override_required_returns_401_with_message` — Staff calls /sessions/:id/close (manager required) without override_pin → 401 + `{code: "override_required", message: "manager"}`.
- `http_login_rate_limit` — 11 rapid /auth/login attempts → 11th returns `429 Too Many Requests` (or whatever status `tower::limit` uses; verify and adjust expectation).

Six tests total. The rig is the bulk of the work; tests are short.

- [ ] **Step 3: Run**

Run: `cd apps/cashier/src-tauri && cargo test --test http_integration`
Expected: 6 pass.

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "test(cashier): HTTP integration — login, auth, lifecycle, WS, override, rate-limit"
```

---

## Task 13: README security posture section

**Files:** Modify `apps/cashier/README.md`

- [ ] **Step 1: Append a "Network Security" section**

```markdown
## Network Security

The cashier exposes its API on `0.0.0.0:7878` (configurable via the
`http_port` setting in `master.db`). Tablets, the cashier's own UI,
and any future external integrations all talk to this endpoint.

**REQUIRED:** the cashier and authorized tablets MUST be on a network
segment isolated from any wifi available to customers. The most common
setup is a router with separate "Staff" and "Guest" SSIDs (any
modern consumer router has this). Without isolation, a customer
on the venue wifi can reach the cashier API directly.

The protocol is plaintext HTTP over TCP. There is no TLS in this
release. Eavesdropping on the staff network would expose tokens
and PINs. Mitigation = strong wifi password + segmented network +
strong staff PINs.

What this codebase does to harden against on-network attackers:

- **Bearer-header auth only.** No cookie auth, so a browser visiting
  a malicious page on the same LAN cannot ride a staff session via
  CSRF.
- **Rate limit on `/auth/login`** (10 attempts/IP/min). Combined with
  Argon2id PIN hashing, online bruteforce of a 6-digit PIN takes years.
- **Bearer tokens expire after 12h** (currently no refresh / no revocation
  list — stolen tablet tokens work for the remainder of TTL).
- **No customer data on the wire** — receipts, names, table assignments
  are visible only to authenticated staff endpoints.

Future hardening (not in this release):
- TLS via self-signed cert with cert pinning in tablet PWA
- Failed-login lockout table in master.db
- Refresh-on-activity sliding TTL with explicit logout / revocation
```

- [ ] **Step 2: Commit**

```bash
git add apps/cashier/README.md
git commit -m "docs(cashier): network security posture; staff-wifi separation required"
```

---

## Task 14: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — all pass
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `pnpm --filter @tableorder/cashier typecheck` clean

---

## Done

End state:
- HTTP+WS is the sole API. 14 routes mirror what used to be 16 Tauri commands plus `/auth/login`, `/auth/me`, `/ws`.
- `commands/` directory deleted. Single source of truth for command pipeline = HTTP route handlers.
- Bearer-header auth only. Rate-limited login. CORS open (safe because no cookies).
- WS notifies subscribers on `WriteOutcome::Inserted`.
- README documents the staff-wifi-separation requirement.

What this unblocks:
- **Plan E1b** writes `packages/shared` (zod + typed fetch + WS hook) and `apps/web` PWA. Cashier React app gets refactored to use the same client.
- Any external client (curl, scripts, kitchen display) drives the cashier via HTTP.
- Plan F's EOD HTTP endpoint is a one-line route addition.

Deferred to a future security plan:
- TLS / cert pinning
- Failed-login lockout
- Token sliding TTL / refresh / revocation
