# First-Run Wizard — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A real venue can install the cashier, launch it, and complete a guided setup (venue identity, Owner PIN, optional initial spots/products) without touching the CLI. Cashier detects "no setup yet" and shows a one-button screen that opens the admin app's `/setup` wizard in the OS browser. After completion the cashier re-detects and transitions to normal PIN login.

**Architecture:**
- New venue-config rows in the existing `setting` key/value table (no new table)
- New unauthenticated endpoint `GET /admin/setup-state` returns `{ needs_setup: bool, lan_url: string }` — `lan_url` is the externally-reachable URL of this cashier (uses LAN IP, not localhost) so a phone on the same Wi-Fi can hit it
- New unauthenticated endpoint `POST /admin/setup` consumes the wizard payload (atomic: writes settings + creates Owner)
- New `@lofi-pos/ui` `LinkQR` component renders any URL as a clickable button + scannable QR code (so a tablet cashier can show the URL to a phone for setup; reusable for receipt URLs, tablet pairing, etc.)
- Admin SPA gains a `/setup` route — when `needs_setup=true`, this is the only accessible route; otherwise normal admin auth applies
- Cashier gains a `SetupRequiredRoute` shown when startup detects `needs_setup=true`; renders the LinkQR for the admin setup URL plus an "Open in browser" button (works on desktop form factor too) and a "Refresh" button
- The existing `cashier seed-owner` CLI subcommand stays as an emergency/automation path but is no longer the primary onboarding flow

**Out of scope (separate plans):**
- Vietnamese localization (next)
- Installer / signing / update channel (next-next)
- Manual smoke + bug fixes from first end-to-end (incremental)

---

## File Structure

```
apps/cashier/src-tauri/src/
  store/migrations/master/
    0009_venue_settings.sql                        # NEW — insert default venue rows
  http/routes/
    setup.rs                                       # NEW — GET /admin/setup-state, POST /admin/setup
                                                   #       (UNAUTHENTICATED while needs_setup=true;
                                                   #        404 once setup is done)
    admin.rs                                       # MODIFIED — Settings schema gets new fields
  http/server.rs                                   # MODIFIED — mount setup routes OUTSIDE the auth layer
  acl/policy.rs                                    # unchanged

packages/shared/src/schemas/
  settings.ts                                      # MODIFIED — add venue_name/address/phone/currency/...
  setup.ts                                         # NEW — zod for SetupRequest + SetupState

apps/cashier/src-tauri/Cargo.toml                  # add tauri-plugin-opener
apps/cashier/src-tauri/capabilities/default.json   # grant opener:allow-open-url
apps/cashier/src-tauri/src/lib.rs                  # init opener plugin

apps/cashier/src-tauri/src/
  net.rs                                           # NEW — LAN IP detection (if-addrs based)
  http/routes/setup.rs                             # MODIFIED — include lan_url in setup-state

packages/ui/src/components/
  link-qr.tsx                                      # NEW — clickable URL with QR code below
                                                   # (uses the qrcode npm package)
packages/ui/package.json                           # MODIFIED — add `qrcode` + `@types/qrcode`

apps/cashier/src/
  routes/
    setup-required.tsx                             # NEW — uses LinkQR; first-run cashier screen
  App.tsx                                          # MODIFIED — branch on setup state before auth gate
  lib/setup.ts                                     # NEW — query setup state; helper to open admin URL

apps/admin/src/
  routes/
    setup.tsx                                      # NEW — wizard UI
  App.tsx                                          # MODIFIED — when needs_setup=true, redirect every
                                                   # path to /setup; otherwise normal flow
```

Decisions:
- **Venue config goes in the `setting` table.** No new table. Keys: `venue_name`, `venue_address`, `venue_phone`, `currency`, `locale`, `tax_id`, `receipt_footer`. Plus the existing `business_day_*`, `idle_lock_minutes`, etc. become editable in the wizard.
- **`needs_setup` is computed**, not stored — derived as `staff WHERE role='owner'` count == 0 OR `venue_name` setting is empty/missing. No "setup complete" flag to keep in sync.
- **`/admin/setup-state` is unauthenticated** so the cashier can probe it before any login. Returns minimal info: just `{needs_setup: bool}`.
- **`/admin/setup` is unauthenticated WHEN `needs_setup=true`** (chicken-and-egg: there's no Owner yet to authorize). Once an Owner exists, the endpoint returns 410 Gone — re-running it would be how you'd reset the system; that requires a different (Owner-authenticated) endpoint we explicitly don't build now.
- **POST is atomic.** Single transaction writes all settings + creates the Owner row. If anything fails, nothing is persisted.
- **Cashier setup-required screen has no idle lock, no nav, no PIN input.** Just headline + explanation + "Open Setup" button + "I've finished setup" refresh button.
- **Owner PIN constraint** — same as today: minimum 6 digits (per `MIN_PIN_LENGTH`).
- **Mock bouncer must be running** for cashier to even reach the setup screen — startup still hard-fails if bouncer is unreachable. That's fine; bouncer is a Tauri sidecar so it boots automatically.

---

## Task 1: Schema migration + setting seed defaults

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0009_venue_settings.sql`

- [ ] **Step 1: migration**

```sql
-- 0009_venue_settings.sql
-- Venue identity / display rows. Empty values are the "needs setup" sentinel.
INSERT OR IGNORE INTO setting(key, value) VALUES
  ('venue_name',     ''),
  ('venue_address',  ''),
  ('venue_phone',    ''),
  ('currency',       'VND'),
  ('locale',         'vi-VN'),
  ('tax_id',         ''),
  ('receipt_footer', '');
```

(Defaults that have one obvious right answer: `currency=VND`, `locale=vi-VN`. Identity fields default to empty so `venue_name == ''` is the "not set up" sentinel.)

- [ ] **Step 2: cargo test migrations_integration**

Existing migration test should still pass; add an assertion that the seven new keys exist after migration with their default values.

- [ ] **Step 3: Commit**

```bash
git add -A
git commit -m "feat(setting): add venue identity rows (default empty / VND / vi-VN)"
```

---

## Task 2: Settings zod + Rust schema extension

**Files:**
- Modify: `packages/shared/src/schemas/settings.ts`
- Modify: `apps/cashier/src-tauri/src/http/routes/admin.rs` (Settings struct + GET /settings + PUT /admin/settings)

- [ ] **Step 1: zod**

```ts
// packages/shared/src/schemas/settings.ts
export const Settings = z.object({
  // Operational (existing)
  business_day_cutoff_hour:        z.number().int().min(0).max(23),
  business_day_tz_offset_seconds:  z.number().int(),
  discount_threshold_pct:          z.number().int().nonnegative(),
  cancel_grace_minutes:            z.number().int().nonnegative(),
  idle_lock_minutes:               z.number().int().positive(),
  // Venue (new)
  venue_name:     z.string(),
  venue_address:  z.string(),
  venue_phone:    z.string(),
  currency:       z.string(),
  locale:         z.string(),
  tax_id:         z.string(),
  receipt_footer: z.string(),
}).strict();
export type Settings = z.infer<typeof Settings>;
```

`SettingsUpdate` (the `PUT /admin/settings` body) becomes `Settings.partial().strict()` so callers can patch any subset.

- [ ] **Step 2: Rust Settings struct mirrors zod**

In whichever file holds the Settings serde struct (likely `services/settings.rs` or `app_state.rs`), add the new string fields. Update `Settings::load(master)` to read them all.

- [ ] **Step 3: tests**

```rust
#[test] fn settings_load_returns_default_venue_values_after_migration() {
    let m = Master::open_in_memory().unwrap();
    let s = Settings::load(&m).unwrap();
    assert_eq!(s.venue_name, "");
    assert_eq!(s.currency, "VND");
    assert_eq!(s.locale, "vi-VN");
}
```

- [ ] **Step 4: cargo test + pnpm typecheck**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(settings): venue_name/address/phone/currency/locale/tax_id/receipt_footer"
```

---

## Task 2.5: LAN IP detection + LinkQR component

**Files:**
- Create: `apps/cashier/src-tauri/src/net.rs`
- Modify: `apps/cashier/src-tauri/Cargo.toml` (add `if-addrs = "0.13"`)
- Create: `packages/ui/src/components/link-qr.tsx`
- Modify: `packages/ui/package.json` (add `qrcode` + `@types/qrcode`)

- [ ] **Step 1: LAN IP detection in Rust**

```rust
// apps/cashier/src-tauri/src/net.rs
use std::net::IpAddr;

/// Best-guess LAN IPv4 for this host. Filters out loopback and link-local;
/// prefers RFC1918 (192.168/16, 10/8, 172.16-31). Returns None if no candidate.
pub fn primary_lan_ipv4() -> Option<String> {
    let ifs = if_addrs::get_if_addrs().ok()?;
    let mut candidates: Vec<IpAddr> = ifs.into_iter()
        .filter(|i| !i.is_loopback())
        .map(|i| i.ip())
        .filter(|ip| matches!(ip, IpAddr::V4(_)))
        .collect();
    // Prefer private addresses
    candidates.sort_by_key(|ip| {
        let v4 = match ip { IpAddr::V4(v) => *v, _ => return 99 };
        let oct = v4.octets();
        match oct {
            [192, 168, _, _] => 0,
            [10, _, _, _]    => 1,
            [172, b, _, _] if (16..=31).contains(&b) => 2,
            [169, 254, _, _] => 90,  // link-local last resort
            _ => 50,
        }
    });
    candidates.first().map(|ip| ip.to_string())
}
```

Tests:
```rust
#[test] fn primary_lan_ipv4_returns_something_or_none_without_panicking() {
    let _ = primary_lan_ipv4();  // smoke
}
```

(Can't assert specific value — depends on host network. Just smoke for no-panic.)

- [ ] **Step 2: LinkQR React component**

```bash
pnpm --filter @lofi-pos/ui add qrcode
pnpm --filter @lofi-pos/ui add -D @types/qrcode
```

```tsx
// packages/ui/src/components/link-qr.tsx
import { useEffect, useRef } from "react";
import QRCode from "qrcode";

interface LinkQRProps {
  url: string;
  /** Show URL text above the QR code; default true */
  showText?: boolean;
  /** QR pixel width; default 192 */
  size?: number;
  /** Optional label e.g. "Scan to open setup on your phone" */
  label?: string;
}

export function LinkQR({ url, showText = true, size = 192, label }: LinkQRProps) {
  const canvas = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    if (canvas.current) {
      void QRCode.toCanvas(canvas.current, url, { width: size, margin: 1 });
    }
  }, [url, size]);
  return (
    <div className="flex flex-col items-center gap-2">
      {label && <div className="text-sm text-gray-600">{label}</div>}
      <a
        href={url}
        target="_blank"
        rel="noreferrer"
        className="text-blue-600 underline break-all text-center text-sm"
      >
        {showText ? url : "Open link"}
      </a>
      <canvas ref={canvas} width={size} height={size} />
    </div>
  );
}
```

Add to `packages/ui/src/index.ts` re-exports if there's a barrel.

- [ ] **Step 3: cargo test net + pnpm --filter @lofi-pos/ui typecheck**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat: LAN IP detection + LinkQR component (URL + QR for cross-device hand-off)"
```

---

## Task 3: setup-state + setup endpoints

**Files:**
- Create: `apps/cashier/src-tauri/src/http/routes/setup.rs`
- Modify: `apps/cashier/src-tauri/src/http/server.rs` (mount under `/admin` BUT outside auth layer)
- Create: `packages/shared/src/schemas/setup.ts`

- [ ] **Step 1: zod schemas**

```ts
// packages/shared/src/schemas/setup.ts
import { z } from "zod";

export const SetupState = z.object({
  needs_setup: z.boolean(),
  /** LAN-reachable URL of this cashier (e.g. http://192.168.1.45:7878),
   *  or http://localhost:7878 if no LAN IP could be resolved. */
  lan_url: z.string().url(),
}).strict();
export type SetupState = z.infer<typeof SetupState>;

export const SetupRequest = z.object({
  // Venue
  venue_name:    z.string().min(1),
  venue_address: z.string(),
  venue_phone:   z.string(),
  currency:      z.string().min(1),
  locale:        z.string().min(1),
  tax_id:        z.string(),
  receipt_footer: z.string(),
  business_day_cutoff_hour:       z.number().int().min(0).max(23),
  business_day_tz_offset_seconds: z.number().int(),
  // Owner account
  owner_name: z.string().min(1),
  owner_pin:  z.string().min(6),
}).strict();
export type SetupRequest = z.infer<typeof SetupRequest>;
```

Re-export from `packages/shared/src/index.ts`.

- [ ] **Step 2: handlers**

```rust
// apps/cashier/src-tauri/src/http/routes/setup.rs
use axum::{extract::State, Json, http::StatusCode};
use serde::{Deserialize, Serialize};
use crate::app_state::AppState;
use crate::auth::pin::hash_pin;
use crate::error::{AppError, AppResult};

#[derive(Serialize)]
struct SetupStateOut { needs_setup: bool, lan_url: String }

pub async fn get_state(State(state): State<Arc<AppState>>) -> AppResult<Json<SetupStateOut>> {
    let needs = compute_needs_setup(&state)?;
    let lan_url = crate::net::primary_lan_ipv4()
        .map(|ip| format!("http://{ip}:{}", state.http_port))
        .unwrap_or_else(|| format!("http://localhost:{}", state.http_port));
    Ok(Json(SetupStateOut { needs_setup: needs, lan_url }))
}

#[derive(Deserialize)]
pub struct SetupBody {
    pub venue_name: String,
    pub venue_address: String,
    pub venue_phone: String,
    pub currency: String,
    pub locale: String,
    pub tax_id: String,
    pub receipt_footer: String,
    pub business_day_cutoff_hour: i64,
    pub business_day_tz_offset_seconds: i64,
    pub owner_name: String,
    pub owner_pin: String,
}

pub async fn submit(State(state): State<Arc<AppState>>, Json(body): Json<SetupBody>) -> AppResult<StatusCode> {
    if !compute_needs_setup(&state)? {
        return Err(AppError::Conflict("setup already complete".into()));
    }
    if body.owner_pin.len() < crate::auth::pin::MIN_PIN_LENGTH {
        return Err(AppError::Validation("owner_pin too short".into()));
    }
    if body.venue_name.trim().is_empty() {
        return Err(AppError::Validation("venue_name required".into()));
    }
    let pin_hash = hash_pin(&body.owner_pin)?;
    let now = state.clock.now_ms();
    {
        let mut conn = state.master.lock().unwrap();
        let tx = conn.transaction()?;
        // Settings
        for (k, v) in &[
            ("venue_name", body.venue_name.as_str()),
            ("venue_address", body.venue_address.as_str()),
            ("venue_phone", body.venue_phone.as_str()),
            ("currency", body.currency.as_str()),
            ("locale", body.locale.as_str()),
            ("tax_id", body.tax_id.as_str()),
            ("receipt_footer", body.receipt_footer.as_str()),
        ] {
            tx.execute(
                "INSERT INTO setting(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![k, v],
            )?;
        }
        for (k, v) in &[
            ("business_day_cutoff_hour", body.business_day_cutoff_hour.to_string()),
            ("business_day_tz_offset_seconds", body.business_day_tz_offset_seconds.to_string()),
        ] {
            tx.execute(
                "INSERT INTO setting(key, value) VALUES (?1, ?2)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                rusqlite::params![k, v],
            )?;
        }
        // Owner
        tx.execute(
            "INSERT INTO staff(name, pin_hash, role, team, created_at)
             VALUES (?1, ?2, 'owner', NULL, ?3)",
            rusqlite::params![body.owner_name, pin_hash, now],
        )?;
        tx.commit()?;
    }
    Ok(StatusCode::CREATED)
}

fn compute_needs_setup(state: &AppState) -> AppResult<bool> {
    let conn = state.master.lock().unwrap();
    let owner_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM staff WHERE role = 'owner'", [], |r| r.get(0),
    )?;
    if owner_count == 0 { return Ok(true); }
    let venue: String = conn.query_row(
        "SELECT value FROM setting WHERE key = 'venue_name'", [], |r| r.get(0),
    ).unwrap_or_default();
    Ok(venue.trim().is_empty())
}

pub fn router() -> axum::Router<Arc<AppState>> {
    axum::Router::new()
        .route("/admin/setup-state", axum::routing::get(get_state))
        .route("/admin/setup", axum::routing::post(submit))
}
```

- [ ] **Step 3: server.rs mount OUTSIDE auth layer**

The setup routes must be reachable without a token. Mount them on the unauthenticated branch of the router.

- [ ] **Step 4: integration tests**

```rust
#[tokio::test] async fn setup_state_returns_true_when_no_owner_or_no_venue_name() { ... }
#[tokio::test] async fn setup_state_returns_false_after_owner_and_venue_name_set() { ... }
#[tokio::test] async fn setup_post_creates_owner_and_writes_settings_atomically() { ... }
#[tokio::test] async fn setup_post_returns_conflict_after_setup_complete() { ... }
#[tokio::test] async fn setup_post_validates_min_pin_length_and_venue_name_required() { ... }
#[tokio::test] async fn setup_endpoints_do_not_require_auth() { ... }
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(http): GET /admin/setup-state + POST /admin/setup (unauthed; gates on owner+venue)"
```

---

## Task 4: Admin SPA setup wizard

**Files:**
- Create: `apps/admin/src/routes/setup.tsx`
- Modify: `apps/admin/src/App.tsx` (route, redirect-when-needs-setup)
- Modify: `apps/admin/src/main.tsx` (probe setup state at boot)

- [ ] **Step 1: setup state probe at app boot**

In `main.tsx` (or a small wrapper), before mounting the normal provider tree, do an unauthenticated `GET /admin/setup-state`. Stash `needs_setup` in a context or just pass into App as a prop.

- [ ] **Step 2: SetupRoute**

```tsx
// apps/admin/src/routes/setup.tsx
// One-page wizard with three sections:
//   1. Venue identity (name, address, phone, currency, locale, tax_id, receipt_footer)
//   2. Operational (cutoff_hour, tz_offset_seconds — defaulted to 11 / 25200 = +07:00)
//   3. Owner (name, PIN, PIN-confirm)
//
// Submit: POST /admin/setup with all fields. On 201, show success + a "You can now return
// to the cashier and click 'I've finished setup'" message + a "Go to admin" button (which
// will hit normal admin login since the system now has Owner).
//
// Use existing @lofi-pos/ui Button + the Modal pattern. No need for fancy step navigation;
// one scrolling form is fine.
```

- [ ] **Step 3: App.tsx routing**

When `needs_setup=true`:
- All routes render the SetupRoute (or `<Navigate to="/setup">` from any other path)
- Skip auth providers / login screen entirely
- Skip the Owner-only gate

When `needs_setup=false`:
- Normal flow (login, AdminShell, owner-only routes)

- [ ] **Step 4: typecheck + build**

```bash
pnpm --filter @lofi-pos/admin typecheck
pnpm --filter @lofi-pos/admin build
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(admin): /setup wizard — venue + operational + owner; routes redirect when needs_setup"
```

---

## Task 5: Cashier first-run screen

**Files:**
- Modify: `apps/cashier/src-tauri/Cargo.toml` (add `tauri-plugin-opener = "2"`)
- Modify: `apps/cashier/src-tauri/capabilities/default.json` (grant `opener:allow-open-url`)
- Modify: `apps/cashier/src-tauri/src/lib.rs` (init opener plugin)
- Create: `apps/cashier/src/lib/setup.ts`
- Create: `apps/cashier/src/routes/setup-required.tsx`
- Modify: `apps/cashier/src/App.tsx`

- [ ] **Step 1: Tauri opener plugin**

```toml
# Cargo.toml
tauri-plugin-opener = "2"
```

```rust
// lib.rs builder
.plugin(tauri_plugin_opener::init())
```

```json
// capabilities/default.json — add to permissions array
"opener:allow-open-url"
```

- [ ] **Step 2: setup state hook (cashier)**

```ts
// apps/cashier/src/lib/setup.ts
import { useQuery } from "@tanstack/react-query";
import { useApiClient } from "@lofi-pos/pos-ui";
import { SetupState } from "@lofi-pos/shared";

export function useSetupState() {
  const client = useApiClient();
  return useQuery({
    queryKey: ["setup-state"],
    queryFn: () => client.get("/admin/setup-state", SetupState),
    refetchOnWindowFocus: true,
  });
}
```

- [ ] **Step 3: SetupRequiredRoute**

```tsx
// apps/cashier/src/routes/setup-required.tsx
import { Button } from "@lofi-pos/ui/components/button";
import { LinkQR } from "@lofi-pos/ui/components/link-qr";
import { useQueryClient } from "@tanstack/react-query";
import { useSetupState } from "../lib/setup";

export function SetupRequiredRoute() {
  const qc = useQueryClient();
  const { data: state } = useSetupState();
  // Use LAN URL from server so a phone/laptop on same Wi-Fi can scan + open
  const lanBase = state?.lan_url ?? window.location.origin;
  const adminSetupUrl = `${lanBase}/ui/admin/#/setup`;

  const openInBrowser = async () => {
    const { openUrl } = await import("@tauri-apps/plugin-opener");
    await openUrl(adminSetupUrl);
  };

  return (
    <div className="min-h-screen flex items-center justify-center bg-gray-50">
      <div className="max-w-lg text-center space-y-6 p-8 bg-white rounded shadow">
        <h1 className="text-2xl font-semibold">First-time setup required</h1>
        <p className="text-gray-600">
          Welcome to LoFi POS. Before staff can log in, the venue owner needs
          to complete a short setup: venue name, currency, and the Owner PIN.
        </p>
        <LinkQR url={adminSetupUrl} label="Scan to open setup on your phone or laptop" />
        <div className="grid grid-cols-2 gap-3">
          <Button onClick={openInBrowser}>Open in browser</Button>
          <Button
            variant="outline"
            onClick={() => qc.invalidateQueries({ queryKey: ["setup-state"] })}
          >
            I&apos;ve finished setup
          </Button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 4: App.tsx branching**

```tsx
import { useSetupState } from "./lib/setup";
import { SetupRequiredRoute } from "./routes/setup-required";

export default function App() {
  const { data: setupState, isLoading } = useSetupState();
  const { isAuthenticated, isLocked, token } = useAuth();

  if (isLoading) return <FullScreenSpinner />;
  if (setupState?.needs_setup) return <SetupRequiredRoute />;

  // existing branches: lock / login / app shell
  ...
}
```

- [ ] **Step 5: typecheck + build**

```bash
pnpm --filter @lofi-pos/cashier typecheck
pnpm --filter @lofi-pos/cashier build
```

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(cashier): first-run screen — opens admin setup in OS browser"
```

---

## Task 6: Final gate

- [ ] `cargo test` — should grow by ~6 (new setup tests)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] All workspace typechecks: shared, pos-ui, cashier, web, admin

- [ ] Manual smoke (highly recommended for this plan since it's UX-heavy):
  ```bash
  # 1. Wipe local DB
  rm -rf ~/Library/Application\ Support/com.lofi-pos.cashier  # or platform equivalent
  # 2. Build sidecar
  bash apps/cashier/src-tauri/scripts/build-sidecars.sh
  # 3. Run cashier
  pnpm --filter @lofi-pos/cashier tauri dev
  # 4. Cashier shows "First-time setup required"
  # 5. Click "Open Admin Setup" → browser opens to /ui/admin/#/setup
  # 6. Fill wizard, submit
  # 7. Return to cashier, click "Refresh"
  # 8. Cashier transitions to PIN login screen
  # 9. Enter Owner PIN, log in successfully
  ```

---

## Done

End state:
- Cashier auto-detects "no setup yet" via `GET /admin/setup-state`
- Owner-friendly first-run screen with one button to open the admin wizard
- Admin `/setup` wizard collects venue identity + operational settings + Owner PIN; submits atomically
- After setup, cashier transitions to normal PIN login on refresh
- The `cashier seed-owner` CLI subcommand still exists for automation/recovery but is no longer the primary onboarding path
- 7 new venue config rows live in the existing `setting` table

Next-up follow-ups (separate plans):
- Vietnamese localization of the cashier UI (locale setting is already there; UI doesn't yet read it)
- Tauri installer / signing / update channel for Windows + macOS
- Manual end-to-end smoke fixing whatever the wizard surfaces
