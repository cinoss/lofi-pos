# Foundation Plan E1d — Tablet PWA + Shared POS UI

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the order-tablet PWA in `apps/web`. Extract the cashier React UI into a shared `@lofi-pos/pos-ui` package so both cashier (Tauri-embedded webview) and the tablet (browser PWA) consume the same components, providers, and pipelines. Add env-driven API base, touch-tuned PinPad, connection-status indicator, PWA manifest, and a basic service worker.

**Architecture:** Move the cashier's React UI primitives (PinPad, OverrideModal, AppShell, AuthProvider, SettingsProvider, useIdleTimer, ApiClient instance factory, ws.ts) into `packages/pos-ui`. Cashier becomes a thin shell: `<App>` + provider tree + routes that consume `pos-ui`. Tablet (`apps/web`) does the same with a different base URL (env var) and a PWA wrapper.

**Builds on:** Plans A through E1c.1.

**Tech stack additions:**
- `vite-plugin-pwa` in `apps/web` (manifest + service worker)
- env var `VITE_API_BASE` consumed in both apps
- Both apps re-import shared UI from `@lofi-pos/pos-ui`

**Out of scope (Plan F):**
- EOD pipeline, reports, print stubs
- Idempotency / denylist pruning
- Spot/staff CRUD admin UI

**Out of scope (deferred):**
- True offline-first (writes while offline). The PWA shell loads cached UI but writes still need server.
- Multi-device session takeover / "log out everywhere"

---

## File Structure

```
packages/pos-ui/                       # NEW workspace
  package.json
  tsconfig.json
  src/
    index.ts                           # public exports
    api.ts                             # ApiClient FACTORY (takes baseUrl)
    ws.ts                              # createEventStreamFactory(baseUrl)
    auth-context.tsx                   # AuthProvider (uses injected ApiClient)
    settings-context.tsx               # SettingsProvider
    idle-tracker.ts                    # useIdleTimer
    components/
      pin-pad.tsx                      # supports `size: "default" | "touch"` prop
      override-modal.tsx
      app-shell.tsx
      connection-status.tsx            # NEW — shows WS state badge
    routes/                            # the ROUTE COMPONENTS, framework-agnostic
      login.tsx
      lock.tsx
      sessions.tsx
      spot-picker.tsx
      session-detail.tsx
      payment.tsx

apps/cashier/                          # consume pos-ui
  src/
    main.tsx                           # MODIFIED — providers from pos-ui
    App.tsx                            # MODIFIED — routes from pos-ui
    index.css                          # unchanged
  package.json                         # MODIFIED — add @lofi-pos/pos-ui

apps/web/                              # NEW tablet PWA
  package.json                         # MODIFIED — add deps + vite-plugin-pwa
  vite.config.ts                       # MODIFIED — pwa plugin + env handling
  index.html                           # MODIFIED — title, manifest link, theme color
  public/
    manifest.webmanifest               # NEW — PWA manifest
    icon-192.png                       # NEW — placeholder icon
    icon-512.png                       # NEW — placeholder icon
  src/
    main.tsx                           # NEW — provider tree + routes
    App.tsx                            # OVERWRITE — same shape as cashier
    index.css                          # NEW — imports @lofi-pos/ui/globals.css
```

Decisions:
- **Package name `@lofi-pos/pos-ui`** (distinct from `@lofi-pos/ui` which is the design-token / shadcn primitives package). pos-ui depends on @lofi-pos/ui + @lofi-pos/shared.
- **`ApiClient` becomes a factory.** Each app calls `createApiClient({ baseUrl, getToken })` once and provides via context. Same for WS (`createEventStreamFactory(baseUrl)`).
- **Routes are framework-agnostic** — they don't import from a specific React Router. They're React components that can be mounted by any router. Since both apps use react-router-dom, this is fine.
- **`VITE_API_BASE`** env var (e.g., `http://localhost:7878` for cashier, `http://10.0.0.5:7878` for tablet). Falls back to `window.location.origin` if unset (tablet only).
- **PinPad `size`** — adds `size?: "default" | "touch"` prop; default keeps current `h-16 w-20`; touch is `h-24 w-28 text-2xl` (tablet finger-friendly).
- **PWA**: minimal manifest, basic service worker via `vite-plugin-pwa` autoUpdate strategy, cache the shell + assets only (no API caching).

---

## Task 1: Scaffold `packages/pos-ui`

**Files:**
- Create: `packages/pos-ui/package.json`
- Create: `packages/pos-ui/tsconfig.json`
- Create: `packages/pos-ui/src/index.ts`

- [ ] **Step 1: package.json**

```json
{
  "name": "@lofi-pos/pos-ui",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "main": "./src/index.ts",
  "types": "./src/index.ts",
  "exports": { ".": "./src/index.ts" },
  "scripts": {
    "typecheck": "tsc --noEmit"
  },
  "dependencies": {
    "@lofi-pos/shared": "workspace:*",
    "@lofi-pos/ui": "workspace:*",
    "@tanstack/react-query": "^5.59.0",
    "react": "^19.2.4",
    "react-dom": "^19.2.4",
    "react-router-dom": "^6.27.0",
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "@types/react": "^19.2.10",
    "@types/react-dom": "^19.2.3",
    "typescript": "5.9.3"
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
    "jsx": "react-jsx",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "skipLibCheck": true,
    "noEmit": true,
    "esModuleInterop": true,
    "verbatimModuleSyntax": true
  },
  "include": ["src"]
}
```

- [ ] **Step 3: stub index.ts**

```ts
export {};
```

- [ ] **Step 4: pnpm install + commit**

```
pnpm install
git add packages/pos-ui pnpm-lock.yaml
git commit -m "feat(pos-ui): scaffold @lofi-pos/pos-ui workspace"
```

---

## Task 2: Move ApiClient + WS factories into pos-ui

**Files:**
- Create: `packages/pos-ui/src/api.ts`, `ws.ts`
- Modify: `apps/cashier/src/lib/api.ts`, `ws.ts` (re-export from pos-ui)

- [ ] **Step 1: pos-ui api.ts**

```ts
import { ApiClient } from "@lofi-pos/shared";

export interface ApiClientConfig {
  baseUrl: string;
  getToken: () => string | null;
}

export function createApiClient(config: ApiClientConfig): ApiClient {
  return new ApiClient(config);
}

export const TOKEN_KEY = "lofi-pos.token";

export function getStoredToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}
export function setStoredToken(token: string | null): void {
  if (token === null) localStorage.removeItem(TOKEN_KEY);
  else localStorage.setItem(TOKEN_KEY, token);
}
```

- [ ] **Step 2: pos-ui ws.ts**

```ts
import { QueryClient } from "@tanstack/react-query";
import { createEventStream, EventNotice } from "@lofi-pos/shared";

export interface WSConfig {
  baseUrl: string;            // e.g. "ws://localhost:7878"
  getToken: () => string | null;
}

export function attachWS(config: WSConfig, queryClient: QueryClient): () => void {
  return createEventStream({
    baseUrl: config.baseUrl,
    getToken: config.getToken,
    onNotice: (n: EventNotice) => {
      queryClient.invalidateQueries({ queryKey: ["sessions", "active"] });
      queryClient.invalidateQueries({ queryKey: ["session", n.aggregate_id] });
      queryClient.invalidateQueries({ queryKey: ["order", n.aggregate_id] });
    },
    onError: (e) => console.warn("ws error", e),
  });
}
```

- [ ] **Step 3: index.ts re-exports**

```ts
export * from "./api";
export * from "./ws";
```

- [ ] **Step 4: cashier api.ts becomes a thin wrapper**

```ts
// apps/cashier/src/lib/api.ts
import { createApiClient, getStoredToken, setStoredToken, TOKEN_KEY } from "@lofi-pos/pos-ui";
import { ApiError } from "@lofi-pos/shared";

const API_BASE = import.meta.env.VITE_API_BASE ?? "http://localhost:7878";
export const apiClient = createApiClient({ baseUrl: API_BASE, getToken: getStoredToken });
export { getStoredToken, setStoredToken, TOKEN_KEY, ApiError, API_BASE };
export const WS_BASE = API_BASE.replace(/^http/, "ws");
```

- [ ] **Step 5: cashier ws.ts becomes a thin wrapper**

```ts
// apps/cashier/src/lib/ws.ts
import { attachWS as attachWSFactory } from "@lofi-pos/pos-ui";
import type { QueryClient } from "@tanstack/react-query";
import { WS_BASE, getStoredToken } from "./api";

export function attachWS(qc: QueryClient): () => void {
  return attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);
}
```

- [ ] **Step 6: pos-ui depends on @tanstack/react-query — already in package.json. typecheck.**

- [ ] **Step 7: Commit**

```
pnpm --filter @lofi-pos/pos-ui typecheck
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "feat(pos-ui): ApiClient + WS factories; cashier delegates"
```

---

## Task 3: Move AuthProvider + SettingsProvider + useIdleTimer into pos-ui

**Files:**
- Create: `packages/pos-ui/src/auth-context.tsx`
- Create: `packages/pos-ui/src/settings-context.tsx`
- Create: `packages/pos-ui/src/idle-tracker.ts`
- Modify: cashier consumers re-export

The pos-ui versions need access to the per-app `ApiClient` instance. Two options:
- (a) Each provider takes the client as a prop
- (b) An `ApiClientContext` wraps everything and providers read it

Pick (a) — simpler, more explicit.

- [ ] **Step 1: AuthProvider in pos-ui takes client as prop**

```tsx
// packages/pos-ui/src/auth-context.tsx
import { createContext, useContext, useEffect, useMemo, useState, useCallback, ReactNode } from "react";
import type { ApiClient, TokenClaims } from "@lofi-pos/shared";
import { TokenClaims as TokenClaimsSchema } from "@lofi-pos/shared";
import { z } from "zod";
import { useQueryClient } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import { setStoredToken, getStoredToken } from "./api";

interface AuthContextValue {
  token: string | null;
  claims: TokenClaims | null;
  login: (pin: string) => Promise<void>;
  logout: () => Promise<void>;
  lock: () => void;
  isAuthenticated: boolean;
  isLocked: boolean;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export interface AuthProviderProps {
  client: ApiClient;
  attachWS: (qc: QueryClient) => () => void;
  children: ReactNode;
}

export function AuthProvider({ client, attachWS, children }: AuthProviderProps) {
  const [token, setToken] = useState<string | null>(getStoredToken());
  const [claims, setClaims] = useState<TokenClaims | null>(null);
  const [isLocked, setIsLocked] = useState(false);
  const qc = useQueryClient();

  useEffect(() => {
    if (!token) return;
    client.get("/auth/me", TokenClaimsSchema).then(setClaims).catch(() => {
      setToken(null);
      setStoredToken(null);
    });
  }, []); // mount only

  useEffect(() => {
    if (!token) return;
    return attachWS(qc);
  }, [token, qc, attachWS]);

  const login = useCallback(async (pin: string) => {
    const out = await client.post("/auth/login", z.object({
      token: z.string(),
      claims: TokenClaimsSchema,
    }), { pin });
    setToken(out.token);
    setStoredToken(out.token);
    setClaims(out.claims);
    setIsLocked(false);
  }, [client]);

  const logout = useCallback(async () => {
    try { await client.post("/auth/logout", z.void()); } catch {}
    setToken(null);
    setClaims(null);
    setStoredToken(null);
    setIsLocked(false);
  }, [client]);

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

- [ ] **Step 2: SettingsProvider in pos-ui takes client as prop**

```tsx
// packages/pos-ui/src/settings-context.tsx
import { createContext, useContext, ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import type { ApiClient } from "@lofi-pos/shared";
import { Settings } from "@lofi-pos/shared";
import type { Settings as SettingsType } from "@lofi-pos/shared";
import { useAuth } from "./auth-context";

const SettingsContext = createContext<SettingsType | null>(null);

export function SettingsProvider({ client, children }: { client: ApiClient; children: ReactNode }) {
  const { isAuthenticated } = useAuth();
  const { data } = useQuery({
    queryKey: ["settings"],
    queryFn: () => client.get("/settings", Settings),
    enabled: isAuthenticated,
    staleTime: 5 * 60 * 1000,
  });
  return <SettingsContext.Provider value={data ?? null}>{children}</SettingsContext.Provider>;
}

export function useSettings(): SettingsType | null {
  return useContext(SettingsContext);
}
```

- [ ] **Step 3: idle-tracker.ts moved verbatim**

```ts
// packages/pos-ui/src/idle-tracker.ts
import { useEffect, useRef } from "react";

const ACTIVITY_EVENTS = ["mousedown", "keydown", "touchstart", "wheel", "mousemove"] as const;

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

- [ ] **Step 4: index.ts adds exports**

```ts
export * from "./api";
export * from "./ws";
export * from "./auth-context";
export * from "./settings-context";
export * from "./idle-tracker";
```

- [ ] **Step 5: cashier removes its lib copies; imports from pos-ui**

Delete `apps/cashier/src/lib/auth-context.tsx`, `settings-context.tsx`, `idle-tracker.ts`. Update consumers (routes/components) to `import { useAuth, useSettings, useIdleTimer, AuthProvider, SettingsProvider } from "@lofi-pos/pos-ui";`.

`apps/cashier/src/main.tsx` constructs the ApiClient and passes into providers:
```tsx
import { AuthProvider, SettingsProvider, attachWS as attachWSFactory } from "@lofi-pos/pos-ui";
import { apiClient, WS_BASE, getStoredToken } from "./lib/api";

const attachWS = (qc: QueryClient) => attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);

// In render:
<AuthProvider client={apiClient} attachWS={attachWS}>
  <SettingsProvider client={apiClient}>
    <BrowserRouter><App /></BrowserRouter>
  </SettingsProvider>
</AuthProvider>
```

- [ ] **Step 6: typecheck + commit**

```
pnpm --filter @lofi-pos/pos-ui typecheck
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "feat(pos-ui): AuthProvider + SettingsProvider + useIdleTimer; cashier consumes"
```

---

## Task 4: Move components into pos-ui (PinPad + OverrideModal + AppShell)

**Files:**
- Create: `packages/pos-ui/src/components/{pin-pad,override-modal,app-shell}.tsx`
- Modify: cashier deletes its copies

PinPad gains a `size` prop; default = current "h-16 w-20"; "touch" = "h-24 w-28 text-2xl".

- [ ] **Step 1: PinPad with size prop**

```tsx
// packages/pos-ui/src/components/pin-pad.tsx
import { useState } from "react";
import { Button } from "@lofi-pos/ui/components/button";
import { MIN_PIN_LEN } from "@lofi-pos/shared";

interface PinPadProps {
  onSubmit: (pin: string) => Promise<void> | void;
  error?: string | null;
  disabled?: boolean;
  title?: string;
  size?: "default" | "touch";
}

const KEYS: Array<string | "back" | "submit"> = [
  "1","2","3","4","5","6","7","8","9","back","0","submit",
];

export function PinPad({ onSubmit, error, disabled, title = "Enter PIN", size = "default" }: PinPadProps) {
  const [pin, setPin] = useState("");
  const press = (k: string) => {
    if (disabled) return;
    if (k === "back") setPin((p) => p.slice(0, -1));
    else if (k === "submit") void onSubmit(pin);
    else if (pin.length < 12) setPin((p) => p + k);
  };
  const buttonClass = size === "touch" ? "h-24 w-28 text-2xl" : "h-16 w-20 text-xl";
  const titleClass = size === "touch" ? "text-3xl" : "text-2xl";
  return (
    <div className="flex flex-col items-center gap-4 p-6">
      <h2 className={`${titleClass} font-semibold`}>{title}</h2>
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
            className={buttonClass}
            onClick={() => press(k)}
            disabled={disabled || (k === "submit" && pin.length < MIN_PIN_LEN)}
          >
            {k === "back" ? "⌫" : k === "submit" ? "✓" : k}
          </Button>
        ))}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: OverrideModal moved verbatim** (with new import path for PinPad)

- [ ] **Step 3: AppShell** — currently uses `react-router-dom`'s `Link` and `Outlet`. Keep — both apps use the same router.

```tsx
// packages/pos-ui/src/components/app-shell.tsx
import { Link, Outlet } from "react-router-dom";
import { Button } from "@lofi-pos/ui/components/button";
import { useAuth } from "../auth-context";
import { useSettings } from "../settings-context";
import { useIdleTimer } from "../idle-tracker";
import { ConnectionStatus } from "./connection-status";

export function AppShell() {
  const { claims, lock, logout } = useAuth();
  const settings = useSettings();
  const idleMs = (settings?.idle_lock_minutes ?? 10) * 60 * 1000;
  useIdleTimer(idleMs, lock);

  return (
    <div className="min-h-screen flex flex-col">
      <header className="flex items-center justify-between border-b bg-white px-6 py-3">
        <Link to="/sessions" className="text-xl font-semibold">LoFi POS</Link>
        <nav className="flex items-center gap-4">
          <Link to="/sessions" className="text-sm hover:underline">Sessions</Link>
          <Link to="/spots" className="text-sm hover:underline">Open New</Link>
          <ConnectionStatus />
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

- [ ] **Step 4: ConnectionStatus** (NEW)

```tsx
// packages/pos-ui/src/components/connection-status.tsx
import { useEffect, useState } from "react";

export function ConnectionStatus() {
  const [online, setOnline] = useState(navigator.onLine);
  useEffect(() => {
    const up = () => setOnline(true);
    const down = () => setOnline(false);
    window.addEventListener("online", up);
    window.addEventListener("offline", down);
    return () => {
      window.removeEventListener("online", up);
      window.removeEventListener("offline", down);
    };
  }, []);
  return (
    <span
      className={`inline-block h-2 w-2 rounded-full ${online ? "bg-green-500" : "bg-red-500"}`}
      title={online ? "Online" : "Offline"}
    />
  );
}
```

(v1: based on `navigator.onLine`. WS-state-aware version is a follow-up.)

- [ ] **Step 5: index.ts adds component exports**

```ts
export * from "./components/pin-pad";
export * from "./components/override-modal";
export * from "./components/app-shell";
export * from "./components/connection-status";
```

- [ ] **Step 6: cashier deletes copies; imports from pos-ui**

Delete `apps/cashier/src/components/{pin-pad,override-modal,app-shell}.tsx`. Update routes (`login.tsx`, `lock.tsx`, etc.) to import from `@lofi-pos/pos-ui`.

- [ ] **Step 7: typecheck + commit**

```
pnpm --filter @lofi-pos/pos-ui typecheck
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "feat(pos-ui): PinPad (with size prop), OverrideModal, AppShell, ConnectionStatus"
```

---

## Task 5: Move routes into pos-ui

**Files:**
- Create: `packages/pos-ui/src/routes/{login,lock,sessions,spot-picker,session-detail,payment}.tsx`
- Modify: `apps/cashier/src/App.tsx` to import routes from pos-ui
- Delete: cashier's local routes

The routes use `apiClient` directly via `import { apiClient } from "../lib/api"`. They need to read it from a context now, since pos-ui doesn't know which app it's in.

Two options:
- (a) Each route component takes `client` as a prop
- (b) Add an `ApiClientContext` in pos-ui, providers wrap, routes consume

Pick (b) — props on every route is too noisy.

- [ ] **Step 1: ApiClientContext**

```tsx
// packages/pos-ui/src/api-context.tsx
import { createContext, useContext, ReactNode } from "react";
import type { ApiClient } from "@lofi-pos/shared";

const ApiClientContext = createContext<ApiClient | null>(null);
export function ApiClientProvider({ client, children }: { client: ApiClient; children: ReactNode }) {
  return <ApiClientContext.Provider value={client}>{children}</ApiClientContext.Provider>;
}
export function useApiClient(): ApiClient {
  const c = useContext(ApiClientContext);
  if (!c) throw new Error("useApiClient outside ApiClientProvider");
  return c;
}
```

- [ ] **Step 2: Move route files verbatim**, replacing `import { apiClient, ApiError } from "../lib/api"` with:
```tsx
import { useApiClient } from "../api-context";
// inside component:
const apiClient = useApiClient();
```

For `ApiError`, import directly from `@lofi-pos/shared`.

- [ ] **Step 3: index.ts adds**

```ts
export * from "./api-context";
export * from "./routes/login";
export * from "./routes/lock";
export * from "./routes/sessions";
export * from "./routes/spot-picker";
export * from "./routes/session-detail";
export * from "./routes/payment";
```

- [ ] **Step 4: cashier App.tsx**

```tsx
import { Navigate, Route, Routes } from "react-router-dom";
import {
  useAuth, AppShell,
  LoginRoute, LockRoute, SessionsRoute, SpotPickerRoute,
  SessionDetailRoute, PaymentRoute,
} from "@lofi-pos/pos-ui";

export default function App() {
  const { isAuthenticated, isLocked, token } = useAuth();
  if (isLocked || (token && !isAuthenticated)) {
    return (
      <Routes>
        <Route path="/lock" element={<LockRoute />} />
        <Route path="*" element={<Navigate to="/lock" replace />} />
      </Routes>
    );
  }
  if (!isAuthenticated) {
    return (
      <Routes>
        <Route path="/login" element={<LoginRoute />} />
        <Route path="*" element={<Navigate to="/login" replace />} />
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

- [ ] **Step 5: cashier main.tsx wraps with ApiClientProvider**

```tsx
import { ApiClientProvider, AuthProvider, SettingsProvider, attachWS as attachWSFactory } from "@lofi-pos/pos-ui";
import { apiClient, WS_BASE, getStoredToken } from "./lib/api";

const attachWS = (qc: QueryClient) => attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);

ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ApiClientProvider client={apiClient}>
        <AuthProvider client={apiClient} attachWS={attachWS}>
          <SettingsProvider client={apiClient}>
            <BrowserRouter><App /></BrowserRouter>
          </SettingsProvider>
        </AuthProvider>
      </ApiClientProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 6: Delete cashier's local routes**

```
git rm apps/cashier/src/routes/{login,lock,sessions,spot-picker,session-detail,payment}.tsx
```

- [ ] **Step 7: typecheck + commit**

```
pnpm --filter @lofi-pos/pos-ui typecheck
pnpm --filter @lofi-pos/cashier typecheck
git add -u
git commit -m "feat(pos-ui): move routes; cashier consumes via ApiClientProvider"
```

---

## Task 6: Stand up `apps/web` tablet PWA

**Files:**
- Modify: `apps/web/package.json` (add deps)
- Modify: `apps/web/vite.config.ts` (add vite-plugin-pwa)
- Modify: `apps/web/index.html` (manifest link, theme-color)
- Create: `apps/web/public/manifest.webmanifest`
- Create: `apps/web/public/icon-192.png`, `icon-512.png` (placeholder; copy from `apps/cashier/src-tauri/icons/icon.png` if needed, or generate solid color)
- Overwrite: `apps/web/src/App.tsx`, `apps/web/src/main.tsx`, `apps/web/src/index.css`

- [ ] **Step 1: web package.json deps**

```json
{
  "dependencies": {
    "@lofi-pos/pos-ui": "workspace:*",
    "@lofi-pos/shared": "workspace:*",
    "@lofi-pos/ui": "workspace:*",
    "@tanstack/react-query": "^5.59.0",
    "react": "^19.2.4",
    "react-dom": "^19.2.4",
    "react-router-dom": "^6.27.0",
    "zod": "^3.23.0"
  },
  "devDependencies": {
    "@tailwindcss/vite": "^4.1.18",
    "@types/react": "^19.2.10",
    "@types/react-dom": "^19.2.3",
    "@vitejs/plugin-react": "^5.1.1",
    "typescript": "5.9.3",
    "vite": "^7.3.2",
    "vite-plugin-pwa": "^0.21.0"
  }
}
```

- [ ] **Step 2: vite.config.ts**

```ts
import path from "path";
import tailwindcss from "@tailwindcss/vite";
import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";
import { VitePWA } from "vite-plugin-pwa";

export default defineConfig({
  plugins: [
    react(),
    tailwindcss(),
    VitePWA({
      registerType: "autoUpdate",
      manifest: {
        name: "LoFi POS — Order Tablet",
        short_name: "LoFi POS",
        theme_color: "#1f2937",
        background_color: "#ffffff",
        display: "standalone",
        icons: [
          { src: "icon-192.png", sizes: "192x192", type: "image/png" },
          { src: "icon-512.png", sizes: "512x512", type: "image/png" },
        ],
      },
      workbox: {
        // Cache the app shell only. API + WS pass through.
        globPatterns: ["**/*.{js,css,html,svg,png}"],
        navigateFallback: "/index.html",
        runtimeCaching: [],
      },
    }),
  ],
  resolve: {
    alias: { "@": path.resolve(__dirname, "./src") },
  },
});
```

- [ ] **Step 3: index.html**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0, maximum-scale=1.0" />
    <meta name="theme-color" content="#1f2937" />
    <link rel="manifest" href="/manifest.webmanifest" />
    <title>LoFi POS — Order Tablet</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 4: index.css**

```css
@import "@lofi-pos/ui/globals.css";
```

- [ ] **Step 5: src/main.tsx**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { ApiClient } from "@lofi-pos/shared";
import {
  ApiClientProvider, AuthProvider, SettingsProvider,
  getStoredToken, attachWS as attachWSFactory,
} from "@lofi-pos/pos-ui";
import "./index.css";
import App from "./App";

const API_BASE = import.meta.env.VITE_API_BASE ?? window.location.origin;
const WS_BASE = API_BASE.replace(/^http/, "ws");

const apiClient = new ApiClient({ baseUrl: API_BASE, getToken: getStoredToken });
const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 1000, refetchOnWindowFocus: false } },
});

const attachWS = (qc: QueryClient) =>
  attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <ApiClientProvider client={apiClient}>
        <AuthProvider client={apiClient} attachWS={attachWS}>
          <SettingsProvider client={apiClient}>
            <BrowserRouter><App /></BrowserRouter>
          </SettingsProvider>
        </AuthProvider>
      </ApiClientProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
```

- [ ] **Step 6: src/App.tsx** (same shape as cashier; same routes)

```tsx
import { Navigate, Route, Routes } from "react-router-dom";
import {
  useAuth, AppShell,
  LoginRoute, LockRoute, SessionsRoute, SpotPickerRoute,
  SessionDetailRoute, PaymentRoute,
} from "@lofi-pos/pos-ui";

export default function App() {
  const { isAuthenticated, isLocked, token } = useAuth();
  if (isLocked || (token && !isAuthenticated)) {
    return <Routes><Route path="*" element={<LockRoute />} /></Routes>;
  }
  if (!isAuthenticated) {
    return <Routes><Route path="*" element={<LoginRoute />} /></Routes>;
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

- [ ] **Step 7: Placeholder PNG icons**

Generate two solid-color 192px and 512px PNGs (or copy the cashier's `icon.png` and resize via any imaging tool). For automation: use `printf` to create a 1×1 valid PNG and rely on browsers stretching, OR create a small Python one-liner.

Quickest: put a 1×1 PNG that the browser will stretch:
```bash
python3 -c "
import struct, zlib, os
def png(path, w, h, color=(31, 41, 55)):
    raw = b''
    for y in range(h):
        raw += b'\\x00' + (bytes(color)) * w
    def chunk(t, d):
        return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d))
    sig = b'\\x89PNG\\r\\n\\x1a\\n'
    ihdr = struct.pack('>IIBBBBB', w, h, 8, 2, 0, 0, 0)
    idat = zlib.compress(raw)
    with open(path, 'wb') as f:
        f.write(sig + chunk(b'IHDR', ihdr) + chunk(b'IDAT', idat) + chunk(b'IEND', b''))
os.makedirs('apps/web/public', exist_ok=True)
png('apps/web/public/icon-192.png', 192, 192)
png('apps/web/public/icon-512.png', 512, 512)
print('icons written')
"
```

- [ ] **Step 8: pnpm install + typecheck**

```
pnpm install
pnpm --filter @lofi-pos/web typecheck
```

- [ ] **Step 9: Verify dev server starts**

```
cd apps/web && pnpm dev   # starts on a default port (e.g. 5173)
# In another terminal:
VITE_API_BASE=http://localhost:7878 pnpm dev   # to point at a running cashier
```

(Manual smoke; no automated test.)

- [ ] **Step 10: Commit**

```
git add -A
git commit -m "feat(web): tablet PWA — manifest, service worker, mounts pos-ui

apps/web is now a thin shell (main.tsx + App.tsx) that consumes the
shared pos-ui package. Same routes as cashier; differs only by:
- env-driven VITE_API_BASE (defaults to window.location.origin)
- PWA manifest + autoUpdate service worker (caches shell, not API)
- Browser-only WebSocket auth (already handled by Plan E1b's
  first-message handshake; no additional client-side change needed)"
```

---

## Task 7: Touch sizing in tablet — opt in to PinPad `size="touch"`

**Files:**
- Modify: pos-ui's `routes/login.tsx`, `routes/lock.tsx` to read a `pinPadSize` from somewhere

Two options:
- (a) New `UIConfigContext` with `pinPadSize: "default" | "touch"`
- (b) Per-app override at the route mount point

(b) is too coupled (route components are shared). (a) is overkill for one knob.

Simpler: detect via `window.matchMedia("(pointer: coarse)")` inside PinPad — if coarse pointer, default to touch. Apps can still pass an explicit `size`.

- [ ] **Step 1: Auto-detect coarse pointer in PinPad**

```tsx
import { useState, useEffect } from "react";
// ...
const [autoTouch, setAutoTouch] = useState(false);
useEffect(() => {
  const mq = window.matchMedia("(pointer: coarse)");
  const update = () => setAutoTouch(mq.matches);
  update();
  mq.addEventListener("change", update);
  return () => mq.removeEventListener("change", update);
}, []);
const effectiveSize = size ?? (autoTouch ? "touch" : "default");
```

Update prop default to `undefined`; resolve at runtime.

- [ ] **Step 2: typecheck + commit**

```
pnpm --filter @lofi-pos/pos-ui typecheck
git add -u
git commit -m "feat(pos-ui): PinPad auto-sizes to touch on coarse-pointer devices"
```

---

## Task 8: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — 177 still pass (no Rust changes)
- [ ] `cargo clippy --all-targets -- -D warnings` clean (no Rust changes)
- [ ] `pnpm --filter @lofi-pos/shared typecheck` clean
- [ ] `pnpm --filter @lofi-pos/pos-ui typecheck` clean
- [ ] `pnpm --filter @lofi-pos/cashier typecheck` clean
- [ ] `pnpm --filter @lofi-pos/web typecheck` clean

Manual smoke (optional):
- `pnpm --filter @lofi-pos/cashier tauri dev` — cashier still works
- `pnpm --filter @lofi-pos/web dev` — tablet boots; visit `http://localhost:<port>?VITE_API_BASE=http://localhost:7878` (or set env)

---

## Done

End state:
- `@lofi-pos/pos-ui` workspace contains every shared piece of cashier UI: ApiClient factory, WS factory, AuthProvider, SettingsProvider, useIdleTimer, PinPad (touch-aware), OverrideModal, AppShell, ConnectionStatus, all 6 routes.
- `apps/cashier` is a thin shell (`main.tsx` providers + `App.tsx` router); zero local UI components or routes.
- `apps/web` is the same shell, plus PWA manifest + autoUpdate service worker. Defaults API base to `window.location.origin`; overridable via `VITE_API_BASE`.
- PinPad auto-detects touch devices via `(pointer: coarse)` and sizes accordingly. Apps can override.

Remaining for Plan F:
- EOD pipeline + reports + print stubs at write sites
- Idempotency-key + token-denylist pruning at EOD
- Spot/staff/product CRUD admin UI (or Tauri CLI extensions)
- True offline mode for tablet (PWA writes while offline) — out of scope today
