# Cashier

Tauri 2 desktop app for the karaoke/bar POS. Owns the local SQLite databases (encrypted at column level), runs the LAN HTTP server (Plan C), and is the only writer to the event store.

## Quickstart

```bash
pnpm install
pnpm --filter @lofi-pos/cashier tauri dev
```

> **Note:** the desktop window currently shows only the placeholder
> `<h1>Cashier</h1>` from `src/App.tsx`. The HTTP API at `:7878` is
> fully wired and testable (see `tests/http_integration.rs`); the
> in-window React UI gets wired in Plan E1b.

First launch generates a random KEK and stores it in the OS keychain (service: `com.lofi-pos.cashier`). The master DB lives at the OS-standard app data dir (`~/Library/Application Support/com.lofi-pos.cashier/master.db` on macOS).

## Layout

- `src/` — React UI (cashier shell, currently scaffold)
- `src-tauri/src/` — Rust core
  - `crypto.rs` — KEK/DEK AES-256-GCM
  - `keychain.rs` — OS keystore abstraction
  - `bootstrap.rs` — first-run KEK initialization
  - `store/` — SQLite layer (`master.rs`, `migrations.rs`, future `events.rs`)
  - `lib.rs` — Tauri entry, `AppState` wiring

## Gates

```bash
cd src-tauri
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

```bash
pnpm --filter @lofi-pos/cashier typecheck
```

See `docs/superpowers/specs/2026-04-27-foundation-design.md` for the full design and `docs/superpowers/plans/` for the implementation plans.

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
- **Argon2id PIN hashing + 6-digit PIN minimum** make online bruteforce
  expensive — at default Argon2 params (~30 ms/verify) a 6-digit PIN
  takes years to bruteforce even unthrottled.
- **Per-IP rate limit on `/auth/login`** via `tower_governor`
  (~10 attempts/IP/min, burst 3). Returns HTTP 429 once exceeded.
  Combined with Argon2id, this cuts online bruteforce from "years" to
  "infeasible" — even a coordinated multi-IP attack hits per-IP limits
  before making meaningful progress.
- **Explicit logout / token revocation.** `POST /auth/logout` denylists
  the calling token's `jti`; subsequent requests with that token return
  401. Backs the idle-lock UI shipped with the cashier window.
- **Bearer tokens expire after 12h.** No silent refresh; an explicit
  `/auth/logout` revokes the token via the master `token_denylist` table.
  A stolen tablet that hasn't been logged out is still usable for the
  remainder of TTL — pair with PIN-on-wake (Plan E1c idle lock).
- **No customer data on the wire** — receipts, names, table assignments
  are visible only to authenticated staff endpoints.

Future hardening (not in this release):
- TLS via self-signed cert with cert pinning in tablet PWA
- Failed-login lockout table in master.db (per-staff, not per-IP)
- Refresh-on-activity sliding TTL
