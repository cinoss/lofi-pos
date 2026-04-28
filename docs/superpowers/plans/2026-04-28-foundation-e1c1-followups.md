# Foundation Plan E1c.1 — Cashier UI Followups

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the carry-overs flagged in Plan E1c's final review so the cashier UI is genuinely usable end-to-end without manual setup. Adds two small backend endpoints (`GET /orders/:id`, `GET /settings`), a `seed-owner` subcommand on the cashier binary (replaces the example file), exports `MIN_PIN_LEN` from shared, fixes WS reconnect on token change, hydrates idle-lock duration from settings, and shows real per-item order detail in the session screen.

**Architecture:** Pure additive. No schema changes, no breaking refactors. Backend gets two read endpoints and a CLI flag; shared exports one constant; frontend rewires three things.

**Builds on:** Plan E1c (cashier React UI MVP).

**Out of scope:** tablet PWA (Plan E1d), EOD pipeline (Plan F), `OverrideRequired` → 403 backend mapping (already done in Plan E1b).

---

## File Structure

```
apps/cashier/src-tauri/src/
  http/routes/
    order.rs               # MODIFIED — add GET /orders/:id
    catalog.rs             # MODIFIED — add GET /settings
  main.rs                  # MODIFIED — accept seed-owner subcommand
  examples/
    seed_owner.rs          # DELETED — replaced by main.rs subcommand

apps/cashier/src/
  lib/
    api.ts                 # MODIFIED — Settings type echo if needed (or import from shared)
    auth-context.tsx       # MODIFIED — WS attach/teardown keyed on token
    settings-context.tsx   # NEW — query GET /settings on auth, expose
    ws.ts                  # MODIFIED — exports attachWS factory only (caller owns lifecycle)
  components/
    app-shell.tsx          # MODIFIED — useIdleTimer reads idle_lock_minutes from settings
    pin-pad.tsx            # MODIFIED — use MIN_PIN_LEN
  routes/
    session-detail.tsx     # MODIFIED — fetch each order via GET /orders/:id, show items, computed subtotal
    payment.tsx            # MODIFIED — default subtotal from session live-subtotal

packages/shared/src/
  schemas/auth.ts          # MODIFIED — export MIN_PIN_LEN constant + apply min(6) to pin fields
  schemas/settings.ts      # NEW — Settings type matching server
  schemas/order.ts         # already correct
  index.ts                 # MODIFIED — re-export new symbols
```

---

## Task 1: Backend — `GET /orders/:id`

**Files:** Modify `apps/cashier/src-tauri/src/http/routes/order.rs`

- [ ] **Step 1: Add route handler**

```rust
async fn get_order(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
    Path(order_id): Path<String>,
) -> Result<Json<crate::domain::order::OrderState>, AppErrorResponse> {
    let s = state.clone();
    let r = tokio::task::spawn_blocking(move || -> Result<crate::domain::order::OrderState, crate::error::AppError> {
        s.commands.load_order(&order_id)?
            .ok_or(crate::error::AppError::NotFound)
    }).await
    .map_err(|e| AppErrorResponse(crate::error::AppError::Internal(format!("join: {e}"))))?
    .map_err(AppErrorResponse)?;
    Ok(Json(r))
}
```

- [ ] **Step 2: Mount route**

In `router()`:
```rust
.route("/orders/:id", get(get_order))
```

(Add `axum::routing::get` import if not present.)

- [ ] **Step 3: Run gate + commit**

```
cargo test
git add -u
git commit -m "feat(cashier): GET /orders/:id returns OrderState"
```

---

## Task 2: Backend — `GET /settings`

**Files:** Modify `apps/cashier/src-tauri/src/http/routes/catalog.rs`

The cached `Settings` struct on `AppState` is what we want to expose. But it's not directly `Serialize`. Quick fix: define a small DTO.

- [ ] **Step 1: Define DTO + handler**

```rust
use serde::Serialize;

#[derive(Serialize)]
pub struct SettingsOut {
    pub business_day_cutoff_hour: u32,
    pub business_day_tz_offset_seconds: i32,
    pub discount_threshold_pct: u32,
    pub cancel_grace_minutes: u32,
    pub idle_lock_minutes: u32,
}

async fn get_settings(
    State(state): State<Arc<AppState>>,
    AuthCtx(_): AuthCtx,
) -> Result<Json<SettingsOut>, AppErrorResponse> {
    let s = &state.settings;
    Ok(Json(SettingsOut {
        business_day_cutoff_hour: s.business_day_cutoff_hour,
        business_day_tz_offset_seconds: s.business_day_tz.local_minus_utc(),
        discount_threshold_pct: s.discount_threshold_pct,
        cancel_grace_minutes: s.cancel_grace_minutes,
        idle_lock_minutes: s.idle_lock_minutes,
    }))
}
```

- [ ] **Step 2: Mount in `router()`**

```rust
.route("/settings", get(get_settings))
```

- [ ] **Step 3: Commit**

```
cargo test
git add -u
git commit -m "feat(cashier): GET /settings exposes business-day + discount + idle-lock settings"
```

---

## Task 3: Backend — `seed-owner` subcommand on the cashier binary

**Files:**
- Modify: `apps/cashier/src-tauri/src/main.rs`
- Delete: `apps/cashier/src-tauri/examples/seed_owner.rs`

The `cashier` bin currently just calls `cashier_lib::run()`. Wrap with a tiny argv check.

- [ ] **Step 1: Update `main.rs`**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::env;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = env::args().collect();
    if args.get(1).is_some_and(|a| a == "seed-owner") {
        if let Err(e) = seed_owner_cmd(&args[2..]) {
            eprintln!("seed-owner failed: {e}");
            std::process::exit(1);
        }
        return;
    }

    cashier_lib::run();
}

fn seed_owner_cmd(args: &[String]) -> Result<(), Box<dyn std::error::Error>> {
    use cashier_lib::acl::Role;
    use cashier_lib::auth::pin::hash_pin;
    use cashier_lib::store::master::Master;
    use std::path::PathBuf;

    let name = args.get(0).cloned().unwrap_or_else(|| "Owner".into());
    let pin = args.get(1).cloned().ok_or(
        "Usage: cashier seed-owner <name> <pin>  (PIN must be >= 6 chars)"
    )?;

    let identifier = "com.lofi-pos.cashier";
    let db_path: PathBuf = if cfg!(target_os = "macos") {
        let home = env::var("HOME")?;
        PathBuf::from(home).join("Library/Application Support").join(identifier)
    } else if cfg!(target_os = "windows") {
        let appdata = env::var("APPDATA")?;
        PathBuf::from(appdata).join(identifier)
    } else {
        let home = env::var("HOME")?;
        PathBuf::from(home).join(".local/share").join(identifier)
    }.join("master.db");

    if !db_path.exists() {
        return Err(format!("master.db not found at {db_path:?} — start the app once first.").into());
    }
    let master = Master::open(&db_path)?;
    let pin_hash = hash_pin(&pin)?;
    let id = master.create_staff(&name, &pin_hash, Role::Owner, None)?;
    println!("✓ Seeded owner staff (id={id}, name={name:?})  Login PIN: {pin}");
    Ok(())
}
```

- [ ] **Step 2: Delete the example**

```
git rm apps/cashier/src-tauri/examples/seed_owner.rs
rmdir apps/cashier/src-tauri/examples 2>/dev/null || true
```

- [ ] **Step 3: Test + commit**

```
cd apps/cashier/src-tauri && cargo build --bin cashier
# (don't actually run seed-owner here — would mutate dev master.db)
git add -u
git commit -m "feat(cashier): seed-owner subcommand replaces examples/ binary"
```

---

## Task 4: Shared — `MIN_PIN_LEN` + Settings schema

**Files:**
- Modify: `packages/shared/src/schemas/auth.ts`
- Create: `packages/shared/src/schemas/settings.ts`
- Modify: `packages/shared/src/index.ts`

- [ ] **Step 1: Add constant + min() to LoginInput**

```ts
// auth.ts
export const MIN_PIN_LEN = 6;
export const LoginInput = z.object({
  pin: z.string().min(MIN_PIN_LEN),
}).strict();
```

- [ ] **Step 2: Create Settings schema**

```ts
// settings.ts
import { z } from "zod";
export const Settings = z.object({
  business_day_cutoff_hour: z.number().int(),
  business_day_tz_offset_seconds: z.number().int(),
  discount_threshold_pct: z.number().int(),
  cancel_grace_minutes: z.number().int(),
  idle_lock_minutes: z.number().int(),
}).strict();
export type Settings = z.infer<typeof Settings>;
```

- [ ] **Step 3: Re-export from index.ts**

```ts
export * from "./schemas/settings";
```

(Plus `MIN_PIN_LEN` is exported transitively via `auth`.)

- [ ] **Step 4: typecheck + commit**

```
pnpm --filter @lofi-pos/shared typecheck
git add -u
git commit -m "feat(shared): export MIN_PIN_LEN and Settings schema"
```

---

## Task 5: UI — WS reconnect on token change

**Files:**
- Modify: `apps/cashier/src/lib/auth-context.tsx`
- Modify: `apps/cashier/src/lib/ws.ts`
- Modify: `apps/cashier/src/main.tsx` (drop module-load attach)

- [ ] **Step 1: Make ws.ts a factory only**

Current `ws.ts::attachWS` is fine; just need callers to control lifecycle. No change needed in `ws.ts` itself.

- [ ] **Step 2: Move attach into AuthProvider effect**

In `auth-context.tsx`, add inside `AuthProvider`:
```tsx
import { useQueryClient } from "@tanstack/react-query";
import { attachWS } from "./ws";

// ... inside the component body:
const qc = useQueryClient();

useEffect(() => {
  if (!token) return;
  const teardown = attachWS(qc);
  return teardown;
}, [token, qc]);
```

- [ ] **Step 3: Drop module-load attach from main.tsx**

Remove these lines:
```tsx
import { attachWS } from "./lib/ws";
let teardownWS: (() => void) | null = null;
if (!teardownWS) teardownWS = attachWS(queryClient);
```

- [ ] **Step 4: typecheck + commit**

```
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "fix(cashier): WS reconnect on token change (was attached at module load with null token)"
```

---

## Task 6: UI — Settings context

**Files:** Create `apps/cashier/src/lib/settings-context.tsx`

- [ ] **Step 1: Implement**

```tsx
import { createContext, useContext, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import { Settings } from "@lofi-pos/shared";
import { apiClient } from "./api";
import { useAuth } from "./auth-context";

const SettingsContext = createContext<Settings | null>(null);

export function SettingsProvider({ children }: { children: ReactNode }) {
  const { isAuthenticated } = useAuth();
  const { data } = useQuery({
    queryKey: ["settings"],
    queryFn: () => apiClient.get("/settings", Settings),
    enabled: isAuthenticated,
    staleTime: 5 * 60 * 1000,
  });
  return <SettingsContext.Provider value={data ?? null}>{children}</SettingsContext.Provider>;
}

export function useSettings(): Settings | null {
  return useContext(SettingsContext);
}
```

- [ ] **Step 2: Wrap in main.tsx**

```tsx
<QueryClientProvider>
  <AuthProvider>
    <SettingsProvider>
      <BrowserRouter><App /></BrowserRouter>
    </SettingsProvider>
  </AuthProvider>
</QueryClientProvider>
```

- [ ] **Step 3: Use in AppShell**

In `app-shell.tsx`:
```tsx
const settings = useSettings();
const idleMs = (settings?.idle_lock_minutes ?? 10) * 60 * 1000;
useIdleTimer(idleMs, lock);
```

(Remove the hard-coded `IDLE_LOCK_MS` constant.)

- [ ] **Step 4: typecheck + commit**

```
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "feat(cashier): SettingsProvider; idle lock duration from server settings"
```

---

## Task 7: UI — PinPad uses MIN_PIN_LEN

**Files:** Modify `apps/cashier/src/components/pin-pad.tsx`

- [ ] **Step 1: Replace literal**

```tsx
import { MIN_PIN_LEN } from "@lofi-pos/shared";
// ...
disabled={disabled || (k === "submit" && pin.length < MIN_PIN_LEN)}
```

- [ ] **Step 2: typecheck + commit**

```
git add -u
git commit -m "chore(cashier): PinPad uses shared MIN_PIN_LEN constant"
```

---

## Task 8: UI — session-detail shows real line items + computed subtotal

**Files:**
- Modify: `apps/cashier/src/routes/session-detail.tsx`
- Modify: `apps/cashier/src/routes/payment.tsx`

- [ ] **Step 1: Fetch each order**

In `session-detail.tsx`, replace the placeholder `useQuery(["session-orders", ...])` with parallel queries:

```tsx
const orderQueries = useQueries({
  queries: (session?.order_ids ?? []).map((oid) => ({
    queryKey: ["order", oid],
    queryFn: () => apiClient.get(`/orders/${oid}`, OrderState),
    enabled: !!session,
  })),
});

const orders = orderQueries
  .map((q) => q.data)
  .filter((o): o is NonNullable<typeof o> => !!o);

const liveSubtotal = orders.reduce((sum, o) => {
  return sum + o.items.reduce((s, it) => {
    if (it.cancelled) return s;
    const netQty = Math.max(0, it.spec.qty - it.returned_qty);
    return s + netQty * it.spec.unit_price;
  }, 0);
}, 0);
```

Render the items in the order list section:
```tsx
<ul className="text-sm">
  {orders.flatMap((o) => o.items.map((it, i) => (
    <li key={`${o.order_id}-${i}`} className={it.cancelled ? "line-through text-gray-400" : ""}>
      {it.spec.qty}× {it.spec.product_name}
      {it.returned_qty > 0 && ` (${it.returned_qty} returned)`}
      <span className="float-right">{(it.spec.qty * it.spec.unit_price).toLocaleString("vi-VN")}đ</span>
    </li>
  )))}
</ul>
<div className="mt-2 pt-2 border-t font-semibold">
  Subtotal: {liveSubtotal.toLocaleString("vi-VN")}đ
</div>
```

Pass the computed subtotal to the payment route via state or URL param. Simpler: read it again on the payment screen via the same query.

- [ ] **Step 2: Payment route uses computed default**

In `payment.tsx`, mirror the orders fetch + compute:

```tsx
const session = useQuery({ queryKey: ["session", sessionId], queryFn: ... }).data;
const orderQueries = useQueries({ queries: (session?.order_ids ?? []).map(...) });
const computed = ...;

const [subtotal, setSubtotal] = useState(0);
useEffect(() => {
  if (subtotal === 0 && computed > 0) setSubtotal(computed);
}, [computed]);
```

(Cashier can override if needed; default lights up once session + orders load.)

- [ ] **Step 3: typecheck + commit**

```
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "feat(cashier): session-detail shows line items + computed subtotal; payment uses it"
```

---

## Task 9: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — all pass (no test count change expected; new endpoints not yet covered by integration tests)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] `pnpm --filter @lofi-pos/cashier typecheck` clean
- [ ] `pnpm --filter @lofi-pos/shared typecheck` clean

---

## Done

End state:
- Backend: `GET /orders/:id`, `GET /settings`, `cashier seed-owner <name> <pin>` subcommand
- Shared: `MIN_PIN_LEN` constant + `Settings` zod schema
- UI: WS attaches+teardowns on token change (live updates work after first login without reload)
- UI: idle lock duration loaded from server (was hard-coded 10min)
- UI: session-detail shows actual line items with names, prices, cancel/return state, computed subtotal
- UI: payment defaults subtotal to the computed value
- UI: PinPad min-length sourced from shared constant

Plan E1d carry-overs still open:
- Tablet PWA (apps/web)
- Touch-tuned PinPad sizing
- env-driven API base
- Connection status indicator

Plan F still owns:
- EOD pipeline + reports + print stubs
- Idempotency + token denylist pruning
- Spot/staff CRUD admin UI (or Tauri CLI)
