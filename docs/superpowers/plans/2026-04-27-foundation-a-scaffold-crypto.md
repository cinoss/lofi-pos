# Foundation Plan A — Scaffold, Crypto, Keychain

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Stand up the cashier Tauri app skeleton, master-DB migration runner, AES-GCM crypto module, and OS keychain integration. End state: `cargo test` passes for crypto + storage unit tests; `pnpm tauri dev` boots an empty cashier window.

**Architecture:** New `apps/cashier` Tauri 2 app (React UI shell + Rust core). Rust core organized as small focused modules: `crypto`, `keychain`, `store::master`, `migrations`. Master DB lives at `app_data_dir/master.db` opened via `rusqlite`. Crypto module exposes `Kek` (loaded from keychain) and `Dek` (random per-day, wrapped with KEK) types with explicit `Drop`/`Zeroize`.

**Tech Stack:** Tauri 2, Rust (rusqlite, aes-gcm, rand, zeroize, keyring, thiserror, tracing, anyhow for tests), React + Vite + shadcn (existing template), pnpm workspace.

**Spec:** `docs/superpowers/specs/2026-04-27-foundation-design.md`

---

## File Structure

```
apps/cashier/                          # NEW Tauri app
  package.json
  index.html
  vite.config.ts
  tsconfig.json
  src/
    main.tsx                           # React entry (placeholder shell)
    App.tsx
  src-tauri/
    Cargo.toml
    tauri.conf.json
    build.rs
    src/
      main.rs                          # Tauri entry; wires modules
      lib.rs                           # exposes modules for tests
      error.rs                         # AppError enum
      keychain.rs                      # keyring-rs wrapper for KEK
      crypto.rs                        # AES-GCM, Kek, Dek, wrap/unwrap
      store/
        mod.rs
        master.rs                      # connection + helpers
        migrations.rs                  # migration runner
        migrations/
          0001_init.sql                # staff, room, table, product, recipe, setting, day_key, daily_report, _migrations
    tests/
      crypto_integration.rs            # round-trip + tamper + wrap
      migrations_integration.rs        # apply, idempotent, schema present
```

Decisions locked here:
- One file per concern. `crypto.rs` < 250 lines; `keychain.rs` < 80 lines; each migration is a single `.sql`.
- Master DB plaintext; only event payloads (Plan B) get column-level encryption.
- `rusqlite` direct (no Diesel/SeaORM) — schema is small, raw SQL is clearer for a POS.
- Tauri 2 (current as of 2026), not Tauri 1.

---

## Task 1: pnpm workspace acknowledges new app

**Files:**
- Modify: `pnpm-workspace.yaml` (already includes `apps/*`, no change needed — verify only)
- Modify: `turbo.json` (verify dev/build/test tasks generic enough — no change expected)

- [ ] **Step 1: Verify workspace globs**

Run: `cat pnpm-workspace.yaml`
Expected output contains: `"apps/*"` and `"packages/*"`

- [ ] **Step 2: Verify turbo tasks**

Run: `cat turbo.json`
Expected: `dev`, `build`, `lint`, `typecheck` tasks present and generic (no app-specific paths).

No commit (verification only).

---

## Task 2: Scaffold `apps/cashier` directory + package.json

**Files:**
- Create: `apps/cashier/package.json`
- Create: `apps/cashier/index.html`
- Create: `apps/cashier/vite.config.ts`
- Create: `apps/cashier/tsconfig.json`
- Create: `apps/cashier/tsconfig.node.json`
- Create: `apps/cashier/.gitignore`
- Create: `apps/cashier/src/main.tsx`
- Create: `apps/cashier/src/App.tsx`

- [ ] **Step 1: Create `apps/cashier/package.json`**

```json
{
  "name": "@tableorder/cashier",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "scripts": {
    "dev": "vite",
    "build": "tsc -b && vite build",
    "preview": "vite preview",
    "tauri": "tauri",
    "typecheck": "tsc -b",
    "lint": "eslint ."
  },
  "dependencies": {
    "@tauri-apps/api": "^2.0.0",
    "@workspace/ui": "workspace:*",
    "react": "^18.3.1",
    "react-dom": "^18.3.1"
  },
  "devDependencies": {
    "@tauri-apps/cli": "^2.0.0",
    "@types/react": "^18.3.12",
    "@types/react-dom": "^18.3.1",
    "@vitejs/plugin-react": "^4.3.4",
    "typescript": "5.9.3",
    "vite": "^5.4.11"
  }
}
```

- [ ] **Step 2: Create `apps/cashier/index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <title>Cashier</title>
  </head>
  <body>
    <div id="root"></div>
    <script type="module" src="/src/main.tsx"></script>
  </body>
</html>
```

- [ ] **Step 3: Create `apps/cashier/vite.config.ts`**

```ts
import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: { port: 1420, strictPort: true },
});
```

- [ ] **Step 4: Create `apps/cashier/tsconfig.json`**

```json
{
  "compilerOptions": {
    "target": "ES2022",
    "lib": ["ES2022", "DOM", "DOM.Iterable"],
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "react-jsx",
    "strict": true,
    "noUncheckedIndexedAccess": true,
    "skipLibCheck": true,
    "noEmit": true,
    "isolatedModules": true,
    "verbatimModuleSyntax": true,
    "resolveJsonModule": true,
    "allowImportingTsExtensions": true
  },
  "include": ["src"],
  "references": [{ "path": "./tsconfig.node.json" }]
}
```

- [ ] **Step 5: Create `apps/cashier/tsconfig.node.json`**

```json
{
  "compilerOptions": {
    "composite": true,
    "skipLibCheck": true,
    "module": "ESNext",
    "moduleResolution": "bundler",
    "allowSyntheticDefaultImports": true
  },
  "include": ["vite.config.ts"]
}
```

- [ ] **Step 6: Create `apps/cashier/.gitignore`**

```
dist/
node_modules/
src-tauri/target/
src-tauri/gen/
```

- [ ] **Step 7: Create `apps/cashier/src/main.tsx`**

```tsx
import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
```

- [ ] **Step 8: Create `apps/cashier/src/App.tsx`**

```tsx
export default function App() {
  return (
    <main className="p-8">
      <h1 className="text-2xl font-bold">Cashier</h1>
      <p className="text-sm text-gray-500">Foundation scaffold.</p>
    </main>
  );
}
```

- [ ] **Step 9: Install deps**

Run: `pnpm install`
Expected: success, new lockfile entries for `apps/cashier`.

- [ ] **Step 10: Verify Vite dev server boots**

Run: `pnpm --filter @tableorder/cashier dev` (Ctrl+C after seeing "Local:")
Expected: `Local: http://localhost:1420/` printed.

- [ ] **Step 11: Commit**

```bash
git add apps/cashier pnpm-lock.yaml
git commit -m "feat(cashier): scaffold Vite+React app shell"
```

---

## Task 3: Scaffold `src-tauri` (Rust crate)

**Files:**
- Create: `apps/cashier/src-tauri/Cargo.toml`
- Create: `apps/cashier/src-tauri/build.rs`
- Create: `apps/cashier/src-tauri/tauri.conf.json`
- Create: `apps/cashier/src-tauri/src/main.rs`
- Create: `apps/cashier/src-tauri/src/lib.rs`
- Create: `apps/cashier/src-tauri/icons/` (placeholder PNGs)

- [ ] **Step 1: Create `Cargo.toml`**

```toml
[package]
name = "cashier"
version = "0.0.1"
edition = "2021"
rust-version = "1.77"

[lib]
name = "cashier_lib"
path = "src/lib.rs"

[[bin]]
name = "cashier"
path = "src/main.rs"

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
tauri = { version = "2", features = [] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "1"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# storage + crypto
rusqlite = { version = "0.32", features = ["bundled"] }
aes-gcm = "0.10"
rand = "0.8"
zeroize = { version = "1.8", features = ["derive"] }
keyring = "3"

[dev-dependencies]
anyhow = "1"
tempfile = "3"
proptest = "1"
```

- [ ] **Step 2: Create `build.rs`**

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 3: Create `tauri.conf.json`**

```json
{
  "$schema": "https://schema.tauri.app/config/2",
  "productName": "Cashier",
  "version": "0.0.1",
  "identifier": "com.lofi-pos.cashier",
  "build": {
    "beforeDevCommand": "pnpm dev",
    "beforeBuildCommand": "pnpm build",
    "devUrl": "http://localhost:1420",
    "frontendDist": "../dist"
  },
  "app": {
    "windows": [
      { "title": "Cashier", "width": 1280, "height": 800, "resizable": true }
    ],
    "security": { "csp": null }
  },
  "bundle": {
    "active": true,
    "targets": "all",
    "icon": ["icons/icon.png"]
  }
}
```

- [ ] **Step 4: Create placeholder icon**

Run: `mkdir -p apps/cashier/src-tauri/icons && printf '\x89PNG\r\n\x1a\n' > apps/cashier/src-tauri/icons/icon.png`
(Real icons added later. Placeholder lets `tauri build` not error on icon presence; dev mode does not require valid icon.)

- [ ] **Step 5: Create `src/main.rs`**

```rust
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();
    cashier_lib::run();
}
```

- [ ] **Step 6: Create `src/lib.rs`**

```rust
pub mod error;
pub mod keychain;
pub mod crypto;
pub mod store;

pub fn run() {
    tauri::Builder::default()
        .setup(|_app| Ok(()))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 7: Create stub `src/error.rs`**

```rust
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("db: {0}")] Db(#[from] rusqlite::Error),
    #[error("crypto: {0}")] Crypto(String),
    #[error("keychain: {0}")] Keychain(String),
    #[error("io: {0}")] Io(#[from] std::io::Error),
    #[error("not found")] NotFound,
    #[error("validation: {0}")] Validation(String),
}

pub type AppResult<T> = Result<T, AppError>;
```

- [ ] **Step 8: Create stub modules so `lib.rs` compiles**

Create `apps/cashier/src-tauri/src/keychain.rs`:
```rust
// filled in Task 4
```

Create `apps/cashier/src-tauri/src/crypto.rs`:
```rust
// filled in Task 5
```

Create `apps/cashier/src-tauri/src/store/mod.rs`:
```rust
pub mod master;
pub mod migrations;
```

Create `apps/cashier/src-tauri/src/store/master.rs`:
```rust
// filled in Task 7
```

Create `apps/cashier/src-tauri/src/store/migrations.rs`:
```rust
// filled in Task 6
```

Create empty dir: `apps/cashier/src-tauri/src/store/migrations/.gitkeep` (run `mkdir -p apps/cashier/src-tauri/src/store/migrations && touch apps/cashier/src-tauri/src/store/migrations/.gitkeep`)

- [ ] **Step 9: Verify Rust compiles**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: success (warnings about empty modules OK).

- [ ] **Step 10: Commit**

```bash
git add apps/cashier/src-tauri
git commit -m "feat(cashier): scaffold Tauri Rust crate with empty modules"
```

---

## Task 4: Implement `keychain` module

**Files:**
- Modify: `apps/cashier/src-tauri/src/keychain.rs`
- Test: inline `#[cfg(test)]` in same file (uses `keyring::mock` to avoid touching real OS keychain in CI)

Approach: define an injectable `KeyStore` trait. Real impl wraps `keyring`; tests use a `HashMap`-backed fake. Avoids OS dependency in unit tests. The `keyring = "3"` dep added in Task 3 is sufficient (default features cover macOS, Windows, and Linux Secret Service).

- [ ] **Step 1: Write the failing test**

In `apps/cashier/src-tauri/src/keychain.rs`:
```rust
use crate::error::{AppError, AppResult};

pub trait KeyStore: Send + Sync {
    fn get(&self, name: &str) -> AppResult<Option<Vec<u8>>>;
    fn set(&self, name: &str, value: &[u8]) -> AppResult<()>;
    fn delete(&self, name: &str) -> AppResult<()>;
}

pub struct OsKeyStore { service: String }

impl OsKeyStore {
    pub fn new(service: impl Into<String>) -> Self { Self { service: service.into() } }
}

impl KeyStore for OsKeyStore {
    fn get(&self, _name: &str) -> AppResult<Option<Vec<u8>>> { unimplemented!() }
    fn set(&self, _name: &str, _value: &[u8]) -> AppResult<()> { unimplemented!() }
    fn delete(&self, _name: &str) -> AppResult<()> { unimplemented!() }
}

#[cfg(test)]
pub mod test_support {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct MemKeyStore(Mutex<HashMap<String, Vec<u8>>>);

    impl KeyStore for MemKeyStore {
        fn get(&self, name: &str) -> AppResult<Option<Vec<u8>>> {
            Ok(self.0.lock().unwrap().get(name).cloned())
        }
        fn set(&self, name: &str, value: &[u8]) -> AppResult<()> {
            self.0.lock().unwrap().insert(name.into(), value.into());
            Ok(())
        }
        fn delete(&self, name: &str) -> AppResult<()> {
            self.0.lock().unwrap().remove(name);
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use test_support::MemKeyStore;

    #[test]
    fn set_get_roundtrip() {
        let ks = MemKeyStore::default();
        ks.set("k", b"hello").unwrap();
        assert_eq!(ks.get("k").unwrap().as_deref(), Some(&b"hello"[..]));
    }

    #[test]
    fn delete_removes() {
        let ks = MemKeyStore::default();
        ks.set("k", b"x").unwrap();
        ks.delete("k").unwrap();
        assert_eq!(ks.get("k").unwrap(), None);
    }

    #[test]
    fn missing_returns_none() {
        let ks = MemKeyStore::default();
        assert_eq!(ks.get("nope").unwrap(), None);
    }
}
```

- [ ] **Step 2: Run tests, verify they pass with stub OsKeyStore**

Run: `cd apps/cashier/src-tauri && cargo test --lib keychain`
Expected: passes (tests use the in-memory backend; OsKeyStore is `unimplemented!()` but no test calls it). Three tests pass.

- [ ] **Step 3: Implement `OsKeyStore` against `keyring`**

Replace the three `unimplemented!()` methods:
```rust
impl KeyStore for OsKeyStore {
    fn get(&self, name: &str) -> AppResult<Option<Vec<u8>>> {
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::Keychain(e.to_string()))?;
        match entry.get_secret() {
            Ok(v) => Ok(Some(v)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(AppError::Keychain(e.to_string())),
        }
    }
    fn set(&self, name: &str, value: &[u8]) -> AppResult<()> {
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::Keychain(e.to_string()))?;
        entry.set_secret(value).map_err(|e| AppError::Keychain(e.to_string()))
    }
    fn delete(&self, name: &str) -> AppResult<()> {
        let entry = keyring::Entry::new(&self.service, name)
            .map_err(|e| AppError::Keychain(e.to_string()))?;
        match entry.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(AppError::Keychain(e.to_string())),
        }
    }
}
```

- [ ] **Step 4: Run tests again**

Run: `cd apps/cashier/src-tauri && cargo test --lib keychain`
Expected: still passes (3 tests, OsKeyStore not exercised in unit tests).

- [ ] **Step 5: Commit**

```bash
git add apps/cashier/src-tauri/src/keychain.rs
git commit -m "feat(cashier): keychain abstraction with OS + in-memory backends"
```

---

## Task 5: Implement `crypto` module — KEK, DEK, AES-GCM round-trip

**Files:**
- Modify: `apps/cashier/src-tauri/src/crypto.rs`
- Test: inline + `apps/cashier/src-tauri/tests/crypto_integration.rs`

- [ ] **Step 1: Write failing unit tests**

Replace `apps/cashier/src-tauri/src/crypto.rs`:
```rust
use crate::error::{AppError, AppResult};
use aes_gcm::{aead::{Aead, KeyInit, Payload}, Aes256Gcm, Nonce};
use rand::RngCore;
use zeroize::{Zeroize, ZeroizeOnDrop};

pub const KEY_LEN: usize = 32;
pub const NONCE_LEN: usize = 12;
pub const TAG_LEN: usize = 16;

#[derive(Clone, ZeroizeOnDrop)]
pub struct Kek([u8; KEY_LEN]);

#[derive(Clone, ZeroizeOnDrop)]
pub struct Dek([u8; KEY_LEN]);

impl Kek {
    pub fn new_random() -> Self {
        let mut k = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut k);
        Self(k)
    }
    pub fn from_bytes(b: &[u8]) -> AppResult<Self> {
        if b.len() != KEY_LEN { return Err(AppError::Crypto("bad kek length".into())); }
        let mut k = [0u8; KEY_LEN]; k.copy_from_slice(b); Ok(Self(k))
    }
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] { &self.0 }

    /// Wrap a DEK with this KEK using AES-GCM. Output: nonce || ct || tag.
    pub fn wrap(&self, dek: &Dek) -> AppResult<Vec<u8>> {
        encrypt(&self.0, dek.as_bytes(), b"dek-wrap")
    }
    pub fn unwrap(&self, blob: &[u8]) -> AppResult<Dek> {
        let pt = decrypt(&self.0, blob, b"dek-wrap")?;
        if pt.len() != KEY_LEN { return Err(AppError::Crypto("bad wrapped dek".into())); }
        let mut k = [0u8; KEY_LEN]; k.copy_from_slice(&pt); Ok(Dek(k))
    }
}

impl Dek {
    pub fn new_random() -> Self {
        let mut k = [0u8; KEY_LEN];
        rand::thread_rng().fill_bytes(&mut k);
        Self(k)
    }
    pub fn as_bytes(&self) -> &[u8; KEY_LEN] { &self.0 }

    /// Encrypt a payload with this DEK. Output: nonce || ct || tag.
    pub fn encrypt(&self, plaintext: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
        encrypt(&self.0, plaintext, aad)
    }
    pub fn decrypt(&self, blob: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
        decrypt(&self.0, blob, aad)
    }
}

fn encrypt(key: &[u8; KEY_LEN], plaintext: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    let cipher = Aes256Gcm::new(key.into());
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::thread_rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, Payload { msg: plaintext, aad })
        .map_err(|e| AppError::Crypto(format!("encrypt: {e}")))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(out)
}

fn decrypt(key: &[u8; KEY_LEN], blob: &[u8], aad: &[u8]) -> AppResult<Vec<u8>> {
    if blob.len() < NONCE_LEN + TAG_LEN { return Err(AppError::Crypto("blob too short".into())); }
    let cipher = Aes256Gcm::new(key.into());
    let (nonce_bytes, ct) = blob.split_at(NONCE_LEN);
    let nonce = Nonce::from_slice(nonce_bytes);
    cipher
        .decrypt(nonce, Payload { msg: ct, aad })
        .map_err(|e| AppError::Crypto(format!("decrypt: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dek_roundtrip() {
        let dek = Dek::new_random();
        let pt = b"hello world";
        let blob = dek.encrypt(pt, b"aad-1").unwrap();
        assert_eq!(dek.decrypt(&blob, b"aad-1").unwrap(), pt);
    }

    #[test]
    fn dek_decrypt_fails_on_wrong_aad() {
        let dek = Dek::new_random();
        let blob = dek.encrypt(b"x", b"a").unwrap();
        assert!(dek.decrypt(&blob, b"b").is_err());
    }

    #[test]
    fn dek_decrypt_fails_on_tamper() {
        let dek = Dek::new_random();
        let mut blob = dek.encrypt(b"x", b"a").unwrap();
        let last = blob.len() - 1;
        blob[last] ^= 0x01;
        assert!(dek.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn dek_decrypt_fails_with_wrong_key() {
        let d1 = Dek::new_random();
        let d2 = Dek::new_random();
        let blob = d1.encrypt(b"x", b"a").unwrap();
        assert!(d2.decrypt(&blob, b"a").is_err());
    }

    #[test]
    fn kek_wrap_unwrap_roundtrip() {
        let kek = Kek::new_random();
        let dek = Dek::new_random();
        let wrapped = kek.wrap(&dek).unwrap();
        let dek2 = kek.unwrap(&wrapped).unwrap();
        assert_eq!(dek.as_bytes(), dek2.as_bytes());
    }

    #[test]
    fn kek_unwrap_with_wrong_kek_fails() {
        let k1 = Kek::new_random();
        let k2 = Kek::new_random();
        let dek = Dek::new_random();
        let wrapped = k1.wrap(&dek).unwrap();
        assert!(k2.unwrap(&wrapped).is_err());
    }

    #[test]
    fn nonce_uniqueness_smoke() {
        // 10k encryptions, no collision (statistical, not exhaustive)
        let dek = Dek::new_random();
        let mut seen = std::collections::HashSet::new();
        for _ in 0..10_000 {
            let blob = dek.encrypt(b"x", b"a").unwrap();
            let nonce = blob[..NONCE_LEN].to_vec();
            assert!(seen.insert(nonce), "nonce collision");
        }
    }

    #[test]
    fn kek_from_bytes_rejects_wrong_length() {
        assert!(Kek::from_bytes(&[0u8; 31]).is_err());
        assert!(Kek::from_bytes(&[0u8; 33]).is_err());
        assert!(Kek::from_bytes(&[0u8; 32]).is_ok());
    }
}
```

- [ ] **Step 2: Run tests, expect them to pass on first compile**

Run: `cd apps/cashier/src-tauri && cargo test --lib crypto`
Expected: 8 tests pass.

(TDD note: in this case the implementation and tests were both written together because the implementation is mechanical AES-GCM. If a test fails, fix code, not test.)

- [ ] **Step 3: Add property test for any-plaintext round-trip**

Add to `crypto.rs` `mod tests`:
```rust
    use proptest::prelude::*;
    proptest! {
        #[test]
        fn prop_dek_roundtrip(pt in proptest::collection::vec(any::<u8>(), 0..4096),
                              aad in proptest::collection::vec(any::<u8>(), 0..256)) {
            let dek = Dek::new_random();
            let blob = dek.encrypt(&pt, &aad).unwrap();
            prop_assert_eq!(dek.decrypt(&blob, &aad).unwrap(), pt);
        }
    }
```

- [ ] **Step 4: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib crypto`
Expected: 9 tests pass (`prop_dek_roundtrip` runs 256 cases by default).

- [ ] **Step 5: Add integration test file**

Create `apps/cashier/src-tauri/tests/crypto_integration.rs`:
```rust
use cashier_lib::crypto::{Dek, Kek};

#[test]
fn full_kek_dek_event_payload_flow() {
    let kek = Kek::new_random();
    let dek = Dek::new_random();
    let wrapped = kek.wrap(&dek).unwrap();

    // Simulate persistence: drop dek, re-derive from wrapped via kek
    drop(dek);
    let dek2 = kek.unwrap(&wrapped).unwrap();

    let payload = serde_json::json!({"type":"OrderPlaced","items":[{"sku":"BIA-1","qty":2}]});
    let bytes = serde_json::to_vec(&payload).unwrap();
    let blob = dek2.encrypt(&bytes, b"event:1").unwrap();
    let pt = dek2.decrypt(&blob, b"event:1").unwrap();
    assert_eq!(pt, bytes);
}
```

- [ ] **Step 6: Run integration test**

Run: `cd apps/cashier/src-tauri && cargo test --test crypto_integration`
Expected: 1 test passes. (Need `serde_json` already in deps — yes, present.)

- [ ] **Step 7: Commit**

```bash
git add apps/cashier/src-tauri/src/crypto.rs apps/cashier/src-tauri/tests/crypto_integration.rs
git commit -m "feat(cashier): AES-GCM crypto with Kek wrap/unwrap and Dek encrypt/decrypt"
```

---

## Task 6: Migration runner

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/migrations.rs`
- Modify: `apps/cashier/src-tauri/Cargo.toml` (add `include_dir = "0.7"`)
- Test: `apps/cashier/src-tauri/tests/migrations_integration.rs`

- [ ] **Step 1: Add `include_dir` to Cargo.toml `[dependencies]`**

```toml
include_dir = "0.7"
```

- [ ] **Step 2: Write failing integration test**

Create `apps/cashier/src-tauri/tests/migrations_integration.rs`:
```rust
use cashier_lib::store::migrations::run_migrations;
use rusqlite::Connection;

#[test]
fn applies_migrations_to_fresh_db() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn).unwrap();
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM _migrations", [], |r| r.get(0),
    ).unwrap();
    assert!(count >= 1, "expected at least one migration applied");
}

#[test]
fn migrations_are_idempotent() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn).unwrap();
    let first: i64 = conn.query_row(
        "SELECT COUNT(*) FROM _migrations", [], |r| r.get(0),
    ).unwrap();
    run_migrations(&mut conn).unwrap();
    let second: i64 = conn.query_row(
        "SELECT COUNT(*) FROM _migrations", [], |r| r.get(0),
    ).unwrap();
    assert_eq!(first, second, "second run should not re-apply");
}
```

- [ ] **Step 3: Run, expect compile failure**

Run: `cd apps/cashier/src-tauri && cargo test --test migrations_integration`
Expected: fail — `run_migrations` doesn't exist.

- [ ] **Step 4: Implement runner**

Replace `apps/cashier/src-tauri/src/store/migrations.rs`:
```rust
use crate::error::{AppError, AppResult};
use include_dir::{include_dir, Dir};
use rusqlite::{Connection, params};

static MIGRATIONS_DIR: Dir<'_> = include_dir!("$CARGO_MANIFEST_DIR/src/store/migrations");

pub fn run_migrations(conn: &mut Connection) -> AppResult<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _migrations (
             name TEXT PRIMARY KEY,
             applied_at INTEGER NOT NULL
         )",
    )?;

    let mut files: Vec<_> = MIGRATIONS_DIR
        .files()
        .filter(|f| f.path().extension().and_then(|s| s.to_str()) == Some("sql"))
        .collect();
    files.sort_by_key(|f| f.path().to_owned());

    for file in files {
        let name = file.path().file_name().and_then(|s| s.to_str())
            .ok_or_else(|| AppError::Validation("bad migration filename".into()))?
            .to_string();
        let applied: Option<i64> = conn
            .query_row("SELECT 1 FROM _migrations WHERE name = ?1", params![name], |r| r.get(0))
            .ok();
        if applied.is_some() { continue; }

        let sql = file.contents_utf8()
            .ok_or_else(|| AppError::Validation(format!("non-utf8 migration {name}")))?;
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.execute(
            "INSERT INTO _migrations(name, applied_at) VALUES (?1, ?2)",
            params![name, chrono_now_ms()],
        )?;
        tx.commit()?;
    }
    Ok(())
}

fn chrono_now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
```

- [ ] **Step 5: Create the first migration**

Create `apps/cashier/src-tauri/src/store/migrations/0001_init.sql`:
```sql
CREATE TABLE staff (
  id          INTEGER PRIMARY KEY AUTOINCREMENT,
  name        TEXT NOT NULL,
  pin_hash    TEXT NOT NULL,
  role        TEXT NOT NULL CHECK (role IN ('staff','cashier','manager','owner')),
  team        TEXT,
  created_at  INTEGER NOT NULL
);

CREATE TABLE room (
  id           INTEGER PRIMARY KEY AUTOINCREMENT,
  name         TEXT NOT NULL UNIQUE,
  hourly_rate  INTEGER NOT NULL,        -- VND, integer
  status       TEXT NOT NULL DEFAULT 'idle'
);

CREATE TABLE "table" (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  name      TEXT NOT NULL UNIQUE,
  room_id   INTEGER REFERENCES room(id) ON DELETE SET NULL,
  status    TEXT NOT NULL DEFAULT 'idle'
);

CREATE TABLE product (
  id     INTEGER PRIMARY KEY AUTOINCREMENT,
  name   TEXT NOT NULL,
  price  INTEGER NOT NULL,              -- VND
  route  TEXT NOT NULL CHECK (route IN ('kitchen','bar','none')),
  kind   TEXT NOT NULL CHECK (kind IN ('item','recipe','time'))
);

CREATE TABLE recipe (
  product_id    INTEGER NOT NULL REFERENCES product(id) ON DELETE CASCADE,
  ingredient_id INTEGER NOT NULL REFERENCES product(id) ON DELETE RESTRICT,
  qty           REAL NOT NULL,           -- grams or units
  unit          TEXT NOT NULL,           -- 'g' | 'unit' | 'ml'
  PRIMARY KEY (product_id, ingredient_id)
);

CREATE TABLE setting (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

INSERT INTO setting(key, value) VALUES
  ('discount_threshold_pct', '10'),
  ('cancel_grace_minutes',   '5'),
  ('idle_lock_minutes',      '10'),
  ('business_day_cutoff_hour', '11');

CREATE TABLE day_key (
  business_day TEXT PRIMARY KEY,         -- 'YYYY-MM-DD'
  wrapped_dek  BLOB NOT NULL,
  created_at   INTEGER NOT NULL
);

CREATE TABLE daily_report (
  business_day            TEXT PRIMARY KEY,
  generated_at            INTEGER NOT NULL,
  order_summary_json      TEXT NOT NULL,
  inventory_summary_json  TEXT NOT NULL
);
```

- [ ] **Step 6: Run integration tests**

Run: `cd apps/cashier/src-tauri && cargo test --test migrations_integration`
Expected: 2 tests pass.

- [ ] **Step 7: Add schema-presence assertion**

Append to `migrations_integration.rs`:
```rust
#[test]
fn expected_tables_exist_after_migration() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn).unwrap();
    let tables = ["staff","room","table","product","recipe","setting","day_key","daily_report","_migrations"];
    for t in tables {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name = ?1",
            rusqlite::params![t], |r| r.get(0),
        ).unwrap();
        assert_eq!(exists, 1, "missing table: {t}");
    }
}

#[test]
fn default_settings_seeded() {
    let mut conn = Connection::open_in_memory().unwrap();
    run_migrations(&mut conn).unwrap();
    let val: String = conn.query_row(
        "SELECT value FROM setting WHERE key='business_day_cutoff_hour'",
        [], |r| r.get(0),
    ).unwrap();
    assert_eq!(val, "11");
}
```

- [ ] **Step 8: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --test migrations_integration`
Expected: 4 tests pass.

- [ ] **Step 9: Commit**

```bash
git add apps/cashier/src-tauri/src/store/migrations.rs apps/cashier/src-tauri/src/store/migrations/0001_init.sql apps/cashier/src-tauri/tests/migrations_integration.rs apps/cashier/src-tauri/Cargo.toml
git commit -m "feat(cashier): SQL migration runner + 0001_init schema"
```

---

## Task 7: Master DB connection helper + day_key CRUD

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/master.rs`
- Test: inline + extend `migrations_integration.rs`

- [ ] **Step 1: Write failing test**

Add to `apps/cashier/src-tauri/src/store/master.rs`:
```rust
use crate::error::{AppError, AppResult};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::Path;

pub struct Master { conn: Connection }

impl Master {
    pub fn open(path: &Path) -> AppResult<Self> {
        let mut conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(&mut conn)?;
        Ok(Self { conn })
    }
    pub fn open_in_memory() -> AppResult<Self> {
        let mut conn = Connection::open_in_memory()?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        crate::store::migrations::run_migrations(&mut conn)?;
        Ok(Self { conn })
    }

    pub fn put_day_key(&self, business_day: &str, wrapped: &[u8]) -> AppResult<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO day_key(business_day, wrapped_dek, created_at) VALUES (?1, ?2, ?3)",
            params![business_day, wrapped, now_ms()],
        )?;
        Ok(())
    }
    pub fn get_day_key(&self, business_day: &str) -> AppResult<Option<Vec<u8>>> {
        Ok(self.conn.query_row(
            "SELECT wrapped_dek FROM day_key WHERE business_day = ?1",
            params![business_day],
            |r| r.get::<_, Vec<u8>>(0),
        ).optional()?)
    }
    pub fn delete_day_key(&self, business_day: &str) -> AppResult<bool> {
        let n = self.conn.execute(
            "DELETE FROM day_key WHERE business_day = ?1",
            params![business_day],
        )?;
        Ok(n > 0)
    }
    pub fn get_setting(&self, key: &str) -> AppResult<Option<String>> {
        Ok(self.conn.query_row(
            "SELECT value FROM setting WHERE key = ?1",
            params![key], |r| r.get::<_, String>(0),
        ).optional()?)
    }
    pub fn set_setting(&self, key: &str, value: &str) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO setting(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_in_memory_runs_migrations() {
        let m = Master::open_in_memory().unwrap();
        assert_eq!(m.get_setting("business_day_cutoff_hour").unwrap().as_deref(), Some("11"));
    }

    #[test]
    fn day_key_put_get_delete() {
        let m = Master::open_in_memory().unwrap();
        assert!(m.get_day_key("2026-04-27").unwrap().is_none());
        m.put_day_key("2026-04-27", &[1,2,3]).unwrap();
        assert_eq!(m.get_day_key("2026-04-27").unwrap(), Some(vec![1,2,3]));
        assert!(m.delete_day_key("2026-04-27").unwrap());
        assert!(m.get_day_key("2026-04-27").unwrap().is_none());
        assert!(!m.delete_day_key("2026-04-27").unwrap()); // already gone
    }

    #[test]
    fn day_key_replace_overwrites() {
        let m = Master::open_in_memory().unwrap();
        m.put_day_key("d", &[1]).unwrap();
        m.put_day_key("d", &[2,2]).unwrap();
        assert_eq!(m.get_day_key("d").unwrap(), Some(vec![2,2]));
    }

    #[test]
    fn setting_upsert() {
        let m = Master::open_in_memory().unwrap();
        m.set_setting("x", "1").unwrap();
        m.set_setting("x", "2").unwrap();
        assert_eq!(m.get_setting("x").unwrap().as_deref(), Some("2"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib store::master`
Expected: 4 tests pass.

- [ ] **Step 3: Commit**

```bash
git add apps/cashier/src-tauri/src/store/master.rs
git commit -m "feat(cashier): Master DB helper with day_key + setting CRUD"
```

---

## Task 8: First-run KEK initialization

**Files:**
- Create: `apps/cashier/src-tauri/src/bootstrap.rs`
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Create `apps/cashier/src-tauri/src/bootstrap.rs`:
```rust
use crate::crypto::Kek;
use crate::error::{AppError, AppResult};
use crate::keychain::KeyStore;

pub const KEK_NAME: &str = "kek";

/// Load existing KEK from keystore or generate + persist a fresh one.
pub fn load_or_init_kek(ks: &dyn KeyStore) -> AppResult<Kek> {
    if let Some(bytes) = ks.get(KEK_NAME)? {
        return Kek::from_bytes(&bytes);
    }
    let kek = Kek::new_random();
    ks.set(KEK_NAME, kek.as_bytes())?;
    Ok(kek)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keychain::test_support::MemKeyStore;

    #[test]
    fn first_run_generates_and_stores_kek() {
        let ks = MemKeyStore::default();
        let kek1 = load_or_init_kek(&ks).unwrap();
        let stored = ks.get(KEK_NAME).unwrap().unwrap();
        assert_eq!(stored, kek1.as_bytes());
    }

    #[test]
    fn second_run_returns_same_kek() {
        let ks = MemKeyStore::default();
        let kek1 = load_or_init_kek(&ks).unwrap();
        let kek2 = load_or_init_kek(&ks).unwrap();
        assert_eq!(kek1.as_bytes(), kek2.as_bytes());
    }

    #[test]
    fn corrupt_stored_kek_returns_error() {
        let ks = MemKeyStore::default();
        ks.set(KEK_NAME, &[0u8; 16]).unwrap(); // wrong length
        assert!(load_or_init_kek(&ks).is_err());
    }
}
```

- [ ] **Step 2: Wire module into `lib.rs`**

Modify `apps/cashier/src-tauri/src/lib.rs`:
```rust
pub mod error;
pub mod keychain;
pub mod crypto;
pub mod store;
pub mod bootstrap;

pub fn run() {
    tauri::Builder::default()
        .setup(|_app| Ok(()))
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 3: Run tests**

Run: `cd apps/cashier/src-tauri && cargo test --lib bootstrap`
Expected: 3 tests pass.

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/bootstrap.rs apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): KEK first-run init + load via keystore"
```

---

## Task 9: Wire bootstrap into Tauri startup (smoke check)

**Files:**
- Modify: `apps/cashier/src-tauri/src/lib.rs`

- [ ] **Step 1: Add startup wiring**

Replace `run` in `apps/cashier/src-tauri/src/lib.rs`:
```rust
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let data_dir = app.path().app_data_dir()
                .expect("no app data dir");
            std::fs::create_dir_all(&data_dir).ok();
            let db_path = data_dir.join("master.db");

            let ks = keychain::OsKeyStore::new("com.lofi-pos.cashier");
            let kek = bootstrap::load_or_init_kek(&ks)
                .expect("failed to load/init KEK");
            let master = store::master::Master::open(&db_path)
                .expect("failed to open master db");

            app.manage(AppState { kek, master: std::sync::Mutex::new(master) });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub struct AppState {
    pub kek: crypto::Kek,
    pub master: std::sync::Mutex<store::master::Master>,
}
```

Add `tauri::Manager` import at top of `lib.rs`:
```rust
use tauri::Manager;
```

- [ ] **Step 2: Verify it compiles**

Run: `cd apps/cashier/src-tauri && cargo check`
Expected: compiles. Some warnings about unused `kek`/`master` fields are fine for now.

- [ ] **Step 3: (Optional, manual) boot the app**

Run: `pnpm --filter @tableorder/cashier tauri dev`
Expected: window opens, "Cashier — Foundation scaffold." visible. Close after verifying.

(This step is optional in CI; gate with `#[cfg(not(test))]` notes for the assistant's manual smoke. Skip if running headless.)

- [ ] **Step 4: Commit**

```bash
git add apps/cashier/src-tauri/src/lib.rs
git commit -m "feat(cashier): wire KEK + master DB into Tauri startup"
```

---

## Task 10: All-tests gate + lint

**Files:** none

- [ ] **Step 1: Run full Rust suite**

Run: `cd apps/cashier/src-tauri && cargo test`
Expected: all unit + integration tests pass.

- [ ] **Step 2: Clippy**

Run: `cd apps/cashier/src-tauri && cargo clippy --all-targets -- -D warnings`
Expected: no warnings. Fix any that appear.

- [ ] **Step 3: Format check**

Run: `cd apps/cashier/src-tauri && cargo fmt --check`
Expected: no diff. Run `cargo fmt` and re-commit if needed.

- [ ] **Step 4: TS typecheck**

Run: `pnpm --filter @tableorder/cashier typecheck`
Expected: passes.

- [ ] **Step 5: Final commit (if anything to fix)**

```bash
git status
# if changes from clippy/fmt:
git add -u && git commit -m "chore(cashier): clippy + fmt"
```

---

## Done

End state:
- `apps/cashier` Tauri app boots with empty React shell.
- Rust core has working: error type, OS keychain abstraction (with in-memory backend for tests), AES-GCM crypto with KEK wrap/unwrap and DEK encrypt/decrypt, SQL migration runner, master DB with `day_key` + `setting` CRUD, KEK first-run bootstrap.
- All units + integration tests green.
- Ready for Plan B: event store + domain projections.
