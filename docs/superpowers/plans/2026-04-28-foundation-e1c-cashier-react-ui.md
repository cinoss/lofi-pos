# Foundation Plan E1c — Cashier React UI MVP

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `apps/cashier/src/App.tsx` (currently `<h1>Cashier</h1>`) with a working React UI for the cashier shell — PIN login, active sessions board, spot picker, session detail (orders + payment), override-PIN modal, idle lock. Uses `@lofi-pos/shared` to call the local HTTP+WS API. Cashier-only this round; tablet PWA = Plan E1d.

**Architecture:** React 18 + React Router + TanStack Query (for HTTP) + the existing `@lofi-pos/ui` (shadcn) components. State: TanStack Query owns server state (cache + invalidation on WS notice); a small auth context (token + claims + last-activity) owns local state. All API calls go through `@lofi-pos/shared`'s `ApiClient`. WS subscription via `createEventStream` triggers query invalidation.

**Tech stack additions (apps/cashier):**
- `@tanstack/react-query` (or just react-query — pick the v5 package)
- `react-router-dom`
- `@lofi-pos/shared` (workspace dep — already exists)
- A few shadcn components added on demand: `dialog`, `input`, `card`, `select`, `toast`

**Spec:** the existing foundation design covers the API; this plan is pure UI wiring.

**Builds on:** A through E1b. No backend changes; only the React side.

**Out of scope (Plan E1d):**
- `apps/web` tablet PWA
- Mobile-touch-optimized UI
- Service worker / install manifest
- Offline shell

**Out of scope (Plan F):**
- EOD trigger button
- Reports view
- Print stub UI

---

## File Structure

```
apps/cashier/
  package.json                         # MODIFIED — deps for react-query, react-router, @lofi-pos/shared
  src/
    App.tsx                            # REWRITTEN — Router + AuthProvider + QueryProvider
    main.tsx                           # MODIFIED — provider wrapping
    lib/
      api.ts                           # NEW — ApiClient instance + query keys + reusable mutators
      auth-context.tsx                 # NEW — token + claims + login/logout + idle tracker
      ws.ts                            # NEW — wire createEventStream → queryClient.invalidate
      idle-tracker.ts                  # NEW — pure logic: last-activity, lock check
    routes/
      login.tsx                        # NEW — PIN pad + login form
      lock.tsx                         # NEW — lock screen (unlock with PIN)
      sessions.tsx                     # NEW — active sessions board
      spot-picker.tsx                  # NEW — list spots, open session
      session-detail.tsx               # NEW — orders + payment + close
      order-entry.tsx                  # NEW — product list + qty stepper + add
      payment.tsx                      # NEW — discount/vat/total/method
    components/
      pin-pad.tsx                      # NEW — numeric on-screen PIN pad
      override-modal.tsx               # NEW — re-prompt for manager PIN on 403
      app-shell.tsx                    # NEW — header (user + lock button) + content slot
      session-card.tsx                 # NEW — session preview tile
packages/ui/                            # may add shadcn dialog/input/card/select/toast as needed
tests/
  (no new Rust tests)
```

Decisions:
- **No new tests in this plan.** UI smoke tests would need a real backend; the HTTP integration test in `apps/cashier/src-tauri/tests/http_integration.rs` already covers the API. UI-side tests deferred to Plan E1e (or skipped pending real users).
- **TanStack Query** (not SWR) — its mutation/invalidation API is best-fit for command-style writes + WS-driven invalidation. v5.
- **No state management library** — Context for auth + Query for server state is enough.
- **Cashier window**: `tauri.conf.json` already loads `http://localhost:1420` in dev (Vite); Vite proxies nothing (we hit `:7878` directly via fetch with `localhost`).
- **Token storage**: localStorage. AuthProvider reads on mount; sets on login; clears on logout/lock-with-revoke.
- **Idle lock**: a `useIdleTimer` hook that listens for window mousemove/keydown; if no activity for `settings.idle_lock_minutes`, navigates to `/lock` (does NOT revoke the token — unlock is a local re-PIN against staff list, no server round-trip until a request fires and 401s).
  - Simpler v1: `settings` aren't yet exposed to the UI. Hard-code 10 minutes; load from server later when there's a `GET /settings` endpoint.

---

## Task 1: Add deps + workspace wiring

**Files:** Modify `apps/cashier/package.json`

- [ ] **Step 1: Add deps**

```json
{
  "dependencies": {
    "@lofi-pos/shared": "workspace:*",
    "@lofi-pos/ui": "workspace:*",
    "@tanstack/react-query": "^5.59.0",
    "@tauri-apps/api": "^2.0.0",
    "react": "^18.3.1",
    "react-dom": "^18.3.1",
    "react-router-dom": "^6.27.0"
  }
}
```

- [ ] **Step 2: Install**

Run: `pnpm install`
Expected: workspace deps resolve.

- [ ] **Step 3: Verify**

Run: `pnpm --filter @lofi-pos/cashier typecheck`
Expected: passes (no source changes yet, but new deps available).

- [ ] **Step 4: Commit**

```bash
git add -u
git commit -m "feat(cashier): add @lofi-pos/shared, react-query, react-router deps"
```

---

## Task 2: ApiClient instance + auth context

**Files:**
- Create: `apps/cashier/src/lib/api.ts`
- Create: `apps/cashier/src/lib/auth-context.tsx`

- [ ] **Step 1: api.ts**

```ts
import { ApiClient, ApiError } from "@lofi-pos/shared";

export const API_BASE = "http://localhost:7878";
export const WS_BASE = "ws://localhost:7878";

export const TOKEN_KEY = "lofi-pos.token";

export function getStoredToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function setStoredToken(token: string | null): void {
  if (token === null) localStorage.removeItem(TOKEN_KEY);
  else localStorage.setItem(TOKEN_KEY, token);
}

export const apiClient = new ApiClient({
  baseUrl: API_BASE,
  getToken: getStoredToken,
});

export { ApiError };
```

- [ ] **Step 2: auth-context.tsx**

```tsx
import { createContext, useContext, useEffect, useMemo, useState, ReactNode, useCallback } from "react";
import {
  TokenClaims, LoginInput, LoginOutput,
  LoginInput as LoginInputSchema, LoginOutput as LoginOutputSchema,
} from "@lofi-pos/shared";
import { apiClient, getStoredToken, setStoredToken } from "./api";

interface AuthContextValue {
  token: string | null;
  claims: TokenClaims | null;
  login: (pin: string) => Promise<void>;
  logout: () => Promise<void>;
  /** Local lock; clears claims but keeps token (unlock by re-PIN). */
  lock: () => void;
  isAuthenticated: boolean;
  isLocked: boolean;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [token, setToken] = useState<string | null>(getStoredToken());
  const [claims, setClaims] = useState<TokenClaims | null>(null);
  const [isLocked, setIsLocked] = useState(false);

  // On mount, if token exists, try /auth/me to recover claims
  useEffect(() => {
    if (!token) return;
    apiClient.get("/auth/me", LoginOutputSchema.shape.claims).then(setClaims).catch(() => {
      // token invalid — clear
      setToken(null);
      setStoredToken(null);
    });
  }, []);  // intentionally only on mount

  const login = useCallback(async (pin: string) => {
    const out = await apiClient.post("/auth/login", LoginOutputSchema, { pin });
    setToken(out.token);
    setStoredToken(out.token);
    setClaims(out.claims);
    setIsLocked(false);
  }, []);

  const logout = useCallback(async () => {
    try { await apiClient.post("/auth/logout", LoginOutputSchema.shape.token.optional() as any); } catch {}
    setToken(null);
    setClaims(null);
    setStoredToken(null);
    setIsLocked(false);
  }, []);

  const lock = useCallback(() => {
    setIsLocked(true);
    setClaims(null);
  }, []);

  const value = useMemo(() => ({
    token, claims, login, logout, lock,
    isAuthenticated: !!token && !isLocked,
    isLocked,
  }), [token, claims, login, logout, lock, isLocked]);

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth outside AuthProvider");
  return ctx;
}
```

(Adjust the `/auth/logout` call — it returns 204 so use `z.void()` schema or call fetch directly.)

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat(cashier): ApiClient + AuthProvider context"
```

---

## Task 3: Query provider + WS-driven invalidation

**Files:**
- Create: `apps/cashier/src/lib/ws.ts`
- Modify: `apps/cashier/src/main.tsx`

- [ ] **Step 1: ws.ts**

```ts
import { QueryClient } from "@tanstack/react-query";
import { createEventStream, EventNotice } from "@lofi-pos/shared";
import { WS_BASE, getStoredToken } from "./api";

/** Wire WS notices to invalidate the relevant query keys. Returns teardown. */
export function attachWS(queryClient: QueryClient): () => void {
  return createEventStream({
    baseUrl: WS_BASE,
    getToken: getStoredToken,
    onNotice: (n: EventNotice) => {
      // Coarse: any write invalidates active sessions + the touched aggregate
      queryClient.invalidateQueries({ queryKey: ["sessions", "active"] });
      queryClient.invalidateQueries({ queryKey: ["session", n.aggregate_id] });
      queryClient.invalidateQueries({ queryKey: ["order", n.aggregate_id] });
    },
    onError: (e) => console.warn("ws error", e),
  });
}
```

- [ ] **Step 2: main.tsx**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import { AuthProvider } from "./lib/auth-context";
import { attachWS } from "./lib/ws";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 1000, refetchOnWindowFocus: false },
  },
});

// Attach WS once the app boots; teardown on unmount (StrictMode runs twice in dev)
let teardownWS: (() => void) | null = null;
const root = document.getElementById("root")!;
if (!teardownWS) teardownWS = attachWS(queryClient);

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </AuthProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat(cashier): QueryClient + WS event-driven invalidation"
```

---

## Task 4: PIN pad component

**Files:** Create `apps/cashier/src/components/pin-pad.tsx`

- [ ] **Step 1: Implement**

```tsx
import { useState } from "react";
import { Button } from "@lofi-pos/ui/components/button";

interface PinPadProps {
  onSubmit: (pin: string) => Promise<void> | void;
  /** Show error message above the pad (e.g., "wrong PIN"). */
  error?: string | null;
  disabled?: boolean;
  title?: string;
}

const KEYS: Array<string | "back" | "submit"> = [
  "1","2","3","4","5","6","7","8","9","back","0","submit",
];

export function PinPad({ onSubmit, error, disabled, title = "Enter PIN" }: PinPadProps) {
  const [pin, setPin] = useState("");

  const press = (k: string) => {
    if (disabled) return;
    if (k === "back") setPin((p) => p.slice(0, -1));
    else if (k === "submit") { void onSubmit(pin); }
    else if (pin.length < 12) setPin((p) => p + k);
  };

  return (
    <div className="flex flex-col items-center gap-4 p-6">
      <h2 className="text-2xl font-semibold">{title}</h2>
      <div className="text-3xl tracking-[0.5em] font-mono h-12">
        {pin.replace(/./g, "•") || <span className="text-gray-400">______</span>}
      </div>
      {error && <div className="text-red-600 text-sm">{error}</div>}
      <div className="grid grid-cols-3 gap-3">
        {KEYS.map((k) => (
          <Button
            key={k}
            variant={k === "submit" ? "default" : k === "back" ? "outline" : "secondary"}
            size="lg"
            className="h-16 w-20 text-xl"
            onClick={() => press(k)}
            disabled={disabled || (k === "submit" && pin.length < 6)}
          >
            {k === "back" ? "⌫" : k === "submit" ? "✓" : k}
          </Button>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add apps/cashier/src/components
git commit -m "feat(cashier): PinPad component"
```

---

## Task 5: Login route

**Files:** Create `apps/cashier/src/routes/login.tsx`

- [ ] **Step 1: Implement**

```tsx
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { PinPad } from "../components/pin-pad";
import { useAuth } from "../lib/auth-context";
import { ApiError } from "../lib/api";

export function LoginRoute() {
  const { login } = useAuth();
  const nav = useNavigate();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const onSubmit = async (pin: string) => {
    setBusy(true); setError(null);
    try {
      await login(pin);
      nav("/sessions", { replace: true });
    } catch (e) {
      if (e instanceof ApiError) {
        if (e.code === "rate_limited") setError("Too many attempts. Try again in a minute.");
        else if (e.isUnauthorized()) setError("Wrong PIN");
        else setError(e.message);
      } else {
        setError("Network error");
      }
    } finally { setBusy(false); }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-gray-50">
      <PinPad title="LoFi POS — Sign In" onSubmit={onSubmit} error={error} disabled={busy} />
    </div>
  );
}
```

- [ ] **Step 2: Commit**

```bash
git add -u
git commit -m "feat(cashier): /login route with PinPad"
```

---

## Task 6: Lock route + idle tracker

**Files:**
- Create: `apps/cashier/src/lib/idle-tracker.ts`
- Create: `apps/cashier/src/routes/lock.tsx`

- [ ] **Step 1: idle-tracker.ts**

```ts
import { useEffect, useRef } from "react";

const ACTIVITY_EVENTS = ["mousedown", "keydown", "touchstart", "wheel", "mousemove"] as const;

/**
 * Calls onIdle when no user activity has occurred for `idleMs`.
 * Resets the timer on any mouse/keyboard/touch event.
 */
export function useIdleTimer(idleMs: number, onIdle: () => void): void {
  const onIdleRef = useRef(onIdle);
  onIdleRef.current = onIdle;

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;

    const reset = () => {
      if (timer) clearTimeout(timer);
      timer = setTimeout(() => onIdleRef.current(), idleMs);
    };
    reset();

    for (const ev of ACTIVITY_EVENTS) window.addEventListener(ev, reset, { passive: true });

    return () => {
      if (timer) clearTimeout(timer);
      for (const ev of ACTIVITY_EVENTS) window.removeEventListener(ev, reset);
    };
  }, [idleMs]);
}
```

- [ ] **Step 2: lock.tsx**

```tsx
import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { PinPad } from "../components/pin-pad";
import { useAuth } from "../lib/auth-context";
import { ApiError } from "../lib/api";

/**
 * Lock screen — shown after idle timeout. PIN entry re-runs login (fresh
 * token, fresh jti). The previous token is left orphaned in localStorage
 * until re-login overwrites it.
 */
export function LockRoute() {
  const { login } = useAuth();
  const nav = useNavigate();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const onSubmit = async (pin: string) => {
    setBusy(true); setError(null);
    try {
      await login(pin);
      nav("/sessions", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized()) setError("Wrong PIN");
      else setError(e instanceof Error ? e.message : "Error");
    } finally { setBusy(false); }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-gray-900">
      <div className="rounded-lg bg-white p-2">
        <PinPad title="🔒 Locked — Re-enter PIN" onSubmit={onSubmit} error={error} disabled={busy} />
      </div>
    </div>
  );
}
```

- [ ] **Step 3: Commit**

```bash
git add -u
git commit -m "feat(cashier): lock screen + useIdleTimer hook"
```

---

## Task 7: App shell + router

**Files:**
- Create: `apps/cashier/src/components/app-shell.tsx`
- Modify: `apps/cashier/src/App.tsx`

- [ ] **Step 1: app-shell.tsx**

```tsx
import { ReactNode } from "react";
import { Link, Outlet } from "react-router-dom";
import { Button } from "@lofi-pos/ui/components/button";
import { useAuth } from "../lib/auth-context";
import { useIdleTimer } from "../lib/idle-tracker";

const IDLE_LOCK_MS = 10 * 60 * 1000;   // hard-coded; load from settings later

export function AppShell() {
  const { claims, lock, logout } = useAuth();
  useIdleTimer(IDLE_LOCK_MS, lock);

  return (
    <div className="min-h-screen flex flex-col">
      <header className="flex items-center justify-between border-b bg-white px-6 py-3">
        <Link to="/sessions" className="text-xl font-semibold">LoFi POS</Link>
        <nav className="flex items-center gap-4">
          <Link to="/sessions" className="text-sm hover:underline">Sessions</Link>
          <Link to="/spots" className="text-sm hover:underline">Open New</Link>
          {claims && (
            <span className="text-sm text-gray-500">
              {claims.role} · staff #{claims.staff_id}
            </span>
          )}
          <Button size="sm" variant="outline" onClick={lock}>Lock</Button>
          <Button size="sm" variant="ghost" onClick={() => void logout()}>Logout</Button>
        </nav>
      </header>
      <main className="flex-1 bg-gray-50 p-6">
        <Outlet />
      </main>
    </div>
  );
}
```

- [ ] **Step 2: App.tsx — Router**

```tsx
import { Navigate, Route, Routes } from "react-router-dom";
import { useAuth } from "./lib/auth-context";
import { AppShell } from "./components/app-shell";
import { LoginRoute } from "./routes/login";
import { LockRoute } from "./routes/lock";
import { SessionsRoute } from "./routes/sessions";
import { SpotPickerRoute } from "./routes/spot-picker";
import { SessionDetailRoute } from "./routes/session-detail";
import { PaymentRoute } from "./routes/payment";

export default function App() {
  const { isAuthenticated, isLocked, token } = useAuth();

  if (!isAuthenticated && !isLocked) {
    return (
      <Routes>
        <Route path="/login" element={<LoginRoute />} />
        <Route path="*" element={<Navigate to="/login" replace />} />
      </Routes>
    );
  }
  if (isLocked || (token && !isAuthenticated)) {
    return (
      <Routes>
        <Route path="/lock" element={<LockRoute />} />
        <Route path="*" element={<Navigate to="/lock" replace />} />
      </Routes>
    );
  }

  return (
    <Routes>
      <Route element={<AppShell />}>
        <Route path="/sessions" element={<SessionsRoute />} />
        <Route path="/spots" element={<SpotPickerRoute />} />
        <Route path="/sessions/:id" element={<SessionDetailRoute />} />
        <Route path="/sessions/:id/payment" element={<PaymentRoute />} />
        <Route path="*" element={<Navigate to="/sessions" replace />} />
      </Route>
    </Routes>
  );
}
```

(Routes referenced but not yet implemented — Task 8-11 fills them in. The plan continues to Task 8 with the routes still unstubbed; recommend creating empty placeholder files OR commit this Task 7 only after Tasks 8-11 land. **Recommended:** commit Tasks 7-11 together as one logical "router + screens" change.)

---

## Task 8: Sessions list route

**Files:** Create `apps/cashier/src/routes/sessions.tsx`

- [ ] **Step 1: Implement**

```tsx
import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { SessionState } from "@lofi-pos/shared";
import { z } from "zod";
import { apiClient } from "../lib/api";

export function SessionsRoute() {
  const { data, isLoading, error } = useQuery({
    queryKey: ["sessions", "active"],
    queryFn: () => apiClient.get("/sessions/active", z.array(SessionState)),
  });

  if (isLoading) return <div>Loading…</div>;
  if (error) return <div className="text-red-600">Error: {String(error)}</div>;

  if (!data || data.length === 0) {
    return (
      <div className="text-center">
        <p className="text-gray-500">No active sessions.</p>
        <Link to="/spots" className="mt-4 inline-block text-blue-600 hover:underline">
          Open new session →
        </Link>
      </div>
    );
  }

  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      {data.map((s) => (
        <Link
          key={s.session_id}
          to={`/sessions/${s.session_id}`}
          className="rounded-lg border bg-white p-4 hover:border-blue-400 hover:shadow"
        >
          <div className="font-semibold">{s.spot.name}</div>
          <div className="text-sm text-gray-500">
            {s.spot.kind === "room" ? "Room" : "Table"} · {s.customer_label ?? "—"}
          </div>
          <div className="mt-2 text-xs text-gray-400">{s.order_ids.length} orders</div>
        </Link>
      ))}
    </div>
  );
}
```

---

## Task 9: Spot picker + open session

**Files:** Create `apps/cashier/src/routes/spot-picker.tsx`

- [ ] **Step 1: Implement**

```tsx
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { Spot, OpenSessionInput, SessionState } from "@lofi-pos/shared";
import { z } from "zod";
import { apiClient } from "../lib/api";
import { Button } from "@lofi-pos/ui/components/button";

function newKey() { return crypto.randomUUID(); }

export function SpotPickerRoute() {
  const qc = useQueryClient();
  const nav = useNavigate();
  const { data: spots } = useQuery({
    queryKey: ["spots"],
    queryFn: () => apiClient.get("/spots", z.array(Spot)),
  });

  const open = useMutation({
    mutationFn: (input: OpenSessionInput) =>
      apiClient.post("/sessions", SessionState, input),
    onSuccess: (session) => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      nav(`/sessions/${session.session_id}`);
    },
  });

  if (!spots) return <div>Loading…</div>;

  const idle = spots.filter((s) => s.status === "idle");

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-4">Open new session</h1>
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
        {idle.map((s) => (
          <Button
            key={s.id}
            variant="outline"
            className="h-24 flex flex-col"
            disabled={open.isPending}
            onClick={() => open.mutate({
              idempotency_key: newKey(),
              spot_id: s.id,
            })}
          >
            <span className="text-lg font-semibold">{s.name}</span>
            <span className="text-xs text-gray-500">{s.kind}</span>
          </Button>
        ))}
      </div>
      {idle.length === 0 && <p className="text-gray-500">No idle spots.</p>}
    </div>
  );
}
```

(Could add a customer-label/team form pre-open; keep simple for v1 — those default to null.)

---

## Task 10: Session detail + order entry

**Files:**
- Create: `apps/cashier/src/routes/session-detail.tsx`
- Create: `apps/cashier/src/routes/order-entry.tsx` (or fold into session-detail)

For v1 let's put order entry inline in session-detail to avoid an extra navigation.

- [ ] **Step 1: session-detail.tsx**

```tsx
import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useParams } from "react-router-dom";
import { z } from "zod";
import { useState } from "react";
import {
  SessionState, OrderState, Product,
  PlaceOrderInput, RawOrderItem,
  CloseSessionInput,
} from "@lofi-pos/shared";
import { apiClient, ApiError } from "../lib/api";
import { Button } from "@lofi-pos/ui/components/button";

function newKey() { return crypto.randomUUID(); }

export function SessionDetailRoute() {
  const { id } = useParams<{ id: string }>();
  const sessionId = id!;
  const qc = useQueryClient();
  const nav = useNavigate();

  const { data: session } = useQuery({
    queryKey: ["session", sessionId],
    queryFn: () => apiClient.get(`/sessions/${sessionId}`, SessionState),
  });

  const { data: products } = useQuery({
    queryKey: ["products"],
    queryFn: () => apiClient.get("/products", z.array(Product)),
  });

  const orders = useQuery({
    queryKey: ["session-orders", sessionId, session?.order_ids ?? []],
    queryFn: async () => {
      if (!session) return [];
      // No bulk endpoint; load each order. Acceptable for typical N <10.
      // (Plan F could add /orders?session_id=...)
      const promises = session.order_ids.map((oid) =>
        // No GET /orders/:id today either; rely on ordering events client-side
        // For v1 we won't show item-level state here; just list order ids.
        Promise.resolve({ order_id: oid }));
      return Promise.all(promises);
    },
    enabled: !!session,
  });

  const placeOrder = useMutation({
    mutationFn: (items: RawOrderItem[]) =>
      apiClient.post("/orders", OrderState, {
        idempotency_key: newKey(),
        session_id: sessionId,
        items,
      } satisfies PlaceOrderInput),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      qc.invalidateQueries({ queryKey: ["session-orders", sessionId] });
      setCart({});
    },
  });

  const closeSession = useMutation({
    mutationFn: () =>
      apiClient.post(`/sessions/${sessionId}/close`, SessionState, {
        idempotency_key: newKey(),
      } satisfies CloseSessionInput),
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      nav("/sessions");
    },
  });

  // Cart state: product_id -> qty
  const [cart, setCart] = useState<Record<number, number>>({});
  const cartItems: RawOrderItem[] = Object.entries(cart)
    .filter(([, qty]) => qty > 0)
    .map(([pid, qty]) => ({ product_id: Number(pid), qty }));

  if (!session) return <div>Loading…</div>;

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
      <section>
        <h1 className="text-2xl font-semibold mb-2">{session.spot.name}</h1>
        <div className="text-sm text-gray-500 mb-4">
          {session.spot.kind} · {session.order_ids.length} orders
        </div>

        <div className="rounded-lg border bg-white p-4 mb-4">
          <h2 className="font-semibold mb-2">Order list</h2>
          {session.order_ids.length === 0 ? (
            <p className="text-gray-500 text-sm">No orders placed yet.</p>
          ) : (
            <ul className="text-sm text-gray-700">
              {session.order_ids.map((oid) => <li key={oid}>· {oid.slice(0, 8)}…</li>)}
            </ul>
          )}
        </div>

        <div className="flex gap-2">
          <Button onClick={() => nav(`/sessions/${sessionId}/payment`)}>Take payment</Button>
          <Button variant="outline" onClick={() => closeSession.mutate()} disabled={closeSession.isPending}>
            Close session
          </Button>
        </div>
        {closeSession.error instanceof ApiError && (
          <div className="text-red-600 text-sm mt-2">{closeSession.error.message}</div>
        )}
      </section>

      <section>
        <h2 className="text-xl font-semibold mb-3">Add items</h2>
        <div className="grid grid-cols-2 gap-2 mb-4">
          {products?.map((p) => (
            <div key={p.id} className="rounded border bg-white p-3 flex items-center justify-between">
              <div>
                <div className="font-medium">{p.name}</div>
                <div className="text-xs text-gray-500">{p.price.toLocaleString("vi-VN")}đ</div>
              </div>
              <div className="flex items-center gap-2">
                <Button size="sm" variant="outline"
                  onClick={() => setCart((c) => ({ ...c, [p.id]: Math.max(0, (c[p.id] ?? 0) - 1) }))}>−</Button>
                <span className="w-8 text-center">{cart[p.id] ?? 0}</span>
                <Button size="sm" variant="outline"
                  onClick={() => setCart((c) => ({ ...c, [p.id]: (c[p.id] ?? 0) + 1 }))}>+</Button>
              </div>
            </div>
          ))}
        </div>
        <Button
          className="w-full"
          disabled={cartItems.length === 0 || placeOrder.isPending}
          onClick={() => placeOrder.mutate(cartItems)}
        >
          Place order ({cartItems.length})
        </Button>
        {placeOrder.error instanceof ApiError && (
          <div className="text-red-600 text-sm mt-2">{placeOrder.error.message}</div>
        )}
      </section>
    </div>
  );
}
```

(Note: GET /orders/:id is missing from the API — flagged in Plan F handoff. v1 just shows order ids without item-level decomposition.)

---

## Task 11: Payment route + override modal

**Files:**
- Create: `apps/cashier/src/components/override-modal.tsx`
- Create: `apps/cashier/src/routes/payment.tsx`

- [ ] **Step 1: override-modal.tsx**

```tsx
import { useState } from "react";
import { PinPad } from "./pin-pad";
import { Button } from "@lofi-pos/ui/components/button";

interface Props {
  /** Role required to authorize, e.g., "manager". Shown in title. */
  minRole: string;
  open: boolean;
  onSubmit: (pin: string) => Promise<void> | void;
  onCancel: () => void;
}

export function OverrideModal({ minRole, open, onSubmit, onCancel }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!open) return null;

  const handle = async (pin: string) => {
    setBusy(true); setError(null);
    try { await onSubmit(pin); }
    catch (e) {
      setError(e instanceof Error ? e.message : "Override rejected");
    } finally { setBusy(false); }
  };

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-4 max-w-sm">
        <PinPad title={`${minRole} override`} onSubmit={handle} error={error} disabled={busy} />
        <Button variant="outline" className="w-full mt-2" onClick={onCancel} disabled={busy}>Cancel</Button>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: payment.tsx**

```tsx
import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { useNavigate, useParams } from "react-router-dom";
import { TakePaymentInput, SessionState } from "@lofi-pos/shared";
import { apiClient, ApiError } from "../lib/api";
import { Button } from "@lofi-pos/ui/components/button";
import { OverrideModal } from "../components/override-modal";

function newKey() { return crypto.randomUUID(); }

export function PaymentRoute() {
  const { id } = useParams<{ id: string }>();
  const sessionId = id!;
  const nav = useNavigate();

  const [subtotal, setSubtotal] = useState(0);
  const [discountPct, setDiscountPct] = useState(0);
  const [vatPct, setVatPct] = useState(8);
  const [method, setMethod] = useState("cash");

  const total = Math.round(subtotal * (100 - discountPct) / 100 * (100 + vatPct) / 100);

  const [pendingInput, setPendingInput] = useState<TakePaymentInput | null>(null);
  const [overrideRole, setOverrideRole] = useState<string | null>(null);

  const pay = useMutation({
    mutationFn: (input: TakePaymentInput) =>
      apiClient.post(`/sessions/${sessionId}/payment`, SessionState, input),
    onSuccess: () => nav("/sessions"),
    onError: (e) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setOverrideRole(e.envelope.message ?? "manager");
      }
    },
  });

  const submit = () => {
    const input: TakePaymentInput = {
      idempotency_key: newKey(),
      subtotal, discount_pct: discountPct, vat_pct: vatPct, total, method,
    };
    setPendingInput(input);
    pay.mutate(input);
  };

  const submitWithOverride = async (overridePin: string) => {
    if (!pendingInput) return;
    await pay.mutateAsync({ ...pendingInput, override_pin: overridePin });
    setOverrideRole(null);
  };

  return (
    <div className="max-w-md mx-auto">
      <h1 className="text-2xl font-semibold mb-4">Take payment</h1>
      <div className="space-y-3">
        <Field label="Subtotal (VND)" value={subtotal} onChange={setSubtotal} />
        <Field label="Discount %" value={discountPct} onChange={setDiscountPct} />
        <Field label="VAT %" value={vatPct} onChange={setVatPct} />
        <div>
          <label className="block text-sm mb-1">Method</label>
          <select className="border rounded px-2 py-1 w-full"
            value={method} onChange={(e) => setMethod(e.target.value)}>
            <option value="cash">Cash</option>
            <option value="card">Card</option>
            <option value="transfer">Bank transfer</option>
          </select>
        </div>
        <div className="text-2xl font-bold pt-2 border-t">
          Total: {total.toLocaleString("vi-VN")}đ
        </div>
        <Button className="w-full" disabled={pay.isPending} onClick={submit}>
          Charge
        </Button>
        {pay.error instanceof ApiError && pay.error.code !== "override_required" && (
          <div className="text-red-600 text-sm">{pay.error.message}</div>
        )}
      </div>
      <OverrideModal
        open={overrideRole !== null}
        minRole={overrideRole ?? "manager"}
        onSubmit={submitWithOverride}
        onCancel={() => setOverrideRole(null)}
      />
    </div>
  );
}

function Field({ label, value, onChange }: { label: string; value: number; onChange: (n: number) => void }) {
  return (
    <div>
      <label className="block text-sm mb-1">{label}</label>
      <input type="number" className="border rounded px-2 py-1 w-full"
        value={value} onChange={(e) => onChange(Number(e.target.value))} />
    </div>
  );
}
```

---

## Task 12: Wire it all together — single commit for routes

After Tasks 7-11 are all in place, commit:

- [ ] **Step 1: typecheck**

Run: `pnpm --filter @lofi-pos/cashier typecheck`
Expected: clean.

- [ ] **Step 2: Try `pnpm --filter @lofi-pos/cashier dev` manually**

If you can spawn the dev server and reach `http://localhost:1420`, verify the login screen renders. If headless, just confirm the server starts.

- [ ] **Step 3: Commit Tasks 7-11 together**

```bash
git add apps/cashier/src
git commit -m "feat(cashier): React UI MVP — login, lock, sessions, spot picker, session detail, payment

Routes: /login, /lock, /sessions, /spots, /sessions/:id, /sessions/:id/payment.
Components: PinPad, OverrideModal, AppShell, SessionCard.
Auth: token in localStorage; AuthProvider exposes login/logout/lock.
Idle lock: 10min hard-coded (load from settings later).
WS: createEventStream invalidates active sessions + per-aggregate queries.
Override flow: 403 from /sessions/:id/payment opens OverrideModal,
re-submits with override_pin."
```

---

## Task 13: Final gate

- [ ] `pnpm --filter @lofi-pos/cashier typecheck` clean
- [ ] `pnpm --filter @lofi-pos/cashier lint` clean
- [ ] `pnpm --filter @lofi-pos/shared typecheck` clean (cashier consumes it)
- [ ] `cd apps/cashier/src-tauri && cargo test` — 177 still pass (no Rust changes)

Optional manual verification:
- `pnpm --filter @lofi-pos/cashier tauri dev` — opens window. Should see login screen.
- (Without staff seeded, login fails. Plan F or admin task seeds initial owner. For now: insert manually via `sqlite3 ~/Library/Application\ Support/com.lofi-pos.cashier/master.db "INSERT INTO staff(name, pin_hash, role, created_at) VALUES('Owner', '<argon2 hash>', 'owner', strftime('%s','now')*1000);"` — flagged in handoff.)

---

## Done

End state: cashier window boots into a usable POS UI:
- PIN login → lands on active sessions board
- WS-driven live refresh
- Spot picker → open session
- Session detail with inline order entry (product list + qty stepper)
- Payment screen with discount/VAT/method + override modal
- Idle lock after 10 min
- Logout button revokes token

What this unblocks:
- **Plan E1d** mirrors the same UI pattern in `apps/web` for tablets, plus PWA manifest + service worker + touch optimization
- **Plan F** adds EOD button + reports + print stubs at write sites + initial owner seeding
- Real users can now drive the cashier end-to-end (after seed)

Known v1 limitations (deferred):
- Order detail (per-item view, cancel/return UI) — needs `GET /orders/:id` endpoint
- `idle_lock_minutes` is hard-coded; needs `GET /settings`
- No initial-owner seeding flow — manual SQL today
- No spot management UI (CRUD on spots) — admin TBD
- No staff management UI — admin TBD
- Subtotal entry is manual; should auto-compute from session orders (needs order detail)
