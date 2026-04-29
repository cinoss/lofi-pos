# UTC Key Rotation — Design

**Status:** Active. A remote-Tor variant was briefly considered and rejected (kills local-first; weaker crypto-shred in practice; centralizes risk; Tor blocking/latency/recovery cost). All stated requirements (Cloak-style key tagging, UTC daily rotation, 3-day retention, cutoff-driven data cleanup) are met locally. The design below assumes it is unrelated to the cryptographic work and can be addressed in a separate spec if it turns out to mean Tor (`arti`) hidden-service exposure.

**Goal:** Decouple data-encryption-key (DEK) lifecycle from the business-day cutoff. Run a key rotation service that produces one DEK per **UTC calendar day** and retains the most recent **3** keys; older keys are deleted (crypto-shred). Data lifecycle (reports, on-disk event row deletion) continues to be driven by the **business-day cutoff** and remains owned by the EOD pipeline.

**Why:** The current model entangles two concerns:
- The cutoff hour is a *reporting* boundary (when does business day D close?).
- Crypto-shred is a *data-protection* control (after how long does plaintext become unrecoverable?).
Today both fire together at the EOD cutoff: the day's report is written and the day's DEK is deleted. That works, but it forces every key-policy decision through the cutoff scheduler and makes the retention window depend on operator behaviour (what happens if EOD is skipped?). Decoupling lets us guarantee a hard 72-hour plaintext lifetime regardless of report status, and lets the rotation service be a small, independently testable component.

---

## What changes

### Today (Plan F)
- `master.day_key` keyed by `business_day` (cutoff-aware string)
- `event.key_id` always equals `event.business_day`
- EOD pipeline at cutoff: write report → delete `day_key` row for yesterday's business_day → events for that business_day become unreadable
- AAD: `{business_day}|{event_type}|{aggregate_id}|{key_id}`

### After this design
- `master.dek` keyed by `utc_day` (`YYYY-MM-DD`, always UTC)
- `event.key_id` = the `utc_day` whose key encrypted this row (independent of `event.business_day`)
- A **key rotation service** (tokio task) wakes at `00:00 UTC` daily and:
  1. Ensures today's UTC DEK exists (idempotent get-or-create)
  2. Deletes any DEK row with `utc_day < (today - 3 days)` (crypto-shred)
- The EOD pipeline (cutoff-driven) keeps doing reports + idempotency/denylist pruning. It NO LONGER touches `dek` rows.
- AAD shape unchanged: `{business_day}|{event_type}|{aggregate_id}|{key_id}` — but `business_day` is now the *reporting* tag and `key_id` is the *crypto* tag, fully independent.

### Cloak parallel
This mirrors how Cloak (Elixir) handles rotation: every ciphertext stores a key tag; encryption always uses the *current* key; decryption looks up the tag's key from a small set of recent keys. Older tagged data becomes unreadable when its key is rotated out.

---

## Components

### 1. KeyManager (was `services::day_key`)
- `KeyManager::current_dek(now: i64) -> Dek` — returns DEK for `utc_day(now)`, get-or-create.
- `KeyManager::dek_for(utc_day: &str) -> Result<Dek>` — lookup; returns `KeyExpired` error if row absent.
- `KeyManager::rotate(now: i64) -> RotationReport` — ensures today's DEK, prunes keys older than 3 days. Idempotent. Used by both the scheduler and a startup catch-up.
- Holds an `Arc<Master>` and an `Arc<Kek>`. No global state; pass into `EventService`.

### 2. RotationScheduler
- A tokio task spawned at app start (alongside the EOD scheduler).
- On start: call `rotate(now)` once (catch-up).
- Loop: compute next `00:00 UTC`, sleep, call `rotate`.
- On settings change: irrelevant — UTC rotation does not depend on cutoff/tz settings.
- Logs each rotation with `(today_utc, deleted_days: Vec<String>)`.

### 3. EventService changes
- Encryption path uses `key_manager.current_dek(now)` and writes `key_id = utc_day(now)`.
- Decryption path uses `key_manager.dek_for(row.key_id)`. If the key is gone (rotated out), surface `AppError::Crypto("key expired")` cleanly. Higher layers (admin reports endpoint, in-memory store warmup) handle that explicitly.

### 4. Schema changes
- New table `dek` (replaces `day_key`):
  ```sql
  CREATE TABLE dek (
    utc_day     TEXT PRIMARY KEY,    -- 'YYYY-MM-DD' UTC
    wrapped_dek BLOB NOT NULL,
    created_at  INTEGER NOT NULL
  );
  ```
- Migration: copy `day_key.business_day → dek.utc_day` if the venue happens to have business_day == utc_day, otherwise drop on import. **Decision:** since this is pre-production and no real data exists, the migration is "drop `day_key`, create `dek`" — no data preservation. (Contrast: a real prod migration would need to re-key historical events. Out of scope.)
- `event.key_id` semantics change but the column is unchanged.

### 5. EOD pipeline changes
- `runner.rs` no longer executes `DELETE FROM day_key WHERE business_day = ?`.
- The crypto-shred guarantee shifts from "after EOD" to "after T+3 UTC days, regardless of EOD".
- Optionally: at EOD, **delete event rows** whose `business_day = closed_day` AND whose report has been written. (Today, rows linger in `event` table forever.) This is the "we clean up data based on cutoff time" part of the brief. See *Data lifecycle* below.

---

## Data lifecycle (post-change)

```
T+0   write event:    event row written, encrypted with utc_day(T+0)'s DEK
T+1*  EOD for business_day D fires (* = local cutoff for D, ≈24h after start of D):
       - report built, daily_report row + reports/D.json written
       - rows in event table where business_day = D are DELETED  (NEW)
       - idempotency_keys + token_denylist pruned
T+3d  rotation service deletes utc_day(T+0)'s DEK
       - any event rows still tagged with that key (e.g., events whose EOD failed
         or hasn't run) become permanently undecryptable
```

Two independent guarantees:
- **Operator guarantee** (cutoff-driven): "after EOD for day D, the events from D are off disk."
- **Crypto guarantee** (UTC-driven): "after 72 hours, no event from that period can be decrypted, regardless of operator action."

Result: even if an attacker steals the master.db today, plaintext recoverable to them is bounded by both the on-disk row retention AND the key TTL.

---

## Failure modes

| Scenario | Behaviour |
|---|---|
| Rotation scheduler not running (e.g., crashed task) | Next app start runs catch-up `rotate(now)` and sweeps any backlog. Until then: today's events still encrypt (because `current_dek` get-or-creates on demand inside the request path), but no pruning happens. Logs warn. |
| Decrypt of a key that's been rotated out | `AppError::Crypto("key expired")`. Reports endpoint returns the row with `summary_json` already on disk; the live `event` table walk returns an error per affected event (handler logs, continues). |
| Clock jumps backwards a day | `current_dek` may briefly create yesterday's UTC key if absent. Harmless — that key is valid for its window. Next forward jump re-uses today's. |
| EOD writes lag rotation by >3 days | Events still on disk become undecryptable. The daily_report row already has the *summarized* JSON; the raw events column was already encrypted-then-written-to-summary, so reports are unaffected. |
| Two processes (Tauri + `cashier eod-now`) hold master.db | WAL allows concurrent reads; writes serialize. Rotation grabs row-level via SQLite locking. Existing pattern. |

---

## ACL / API impact

No new HTTP endpoints. The rotation service is purely internal.

For ops visibility, add **`GET /admin/keys`** (Owner-only) returning `[{utc_day, created_at}]` for the keys currently held. Lets an admin see "yes, rotation ran today." Optional in v1.

---

## Testing

Unit:
- `utc_day_of(ts_ms) == "YYYY-MM-DD"` for boundary cases (epoch, just before/after midnight UTC).
- `KeyManager::current_dek` is idempotent; concurrent callers get the same DEK (race-safe via `INSERT OR IGNORE` then re-read).
- `KeyManager::rotate` deletes only keys older than 3 days; never deletes today's; idempotent re-run is a no-op.
- `EventService::encrypt` tags `key_id = utc_day(now)`, *not* `business_day`.
- `EventService::decrypt` of a row whose key has been rotated out returns `AppError::Crypto("key expired")` (not a panic, not a NotFound).

Integration:
- Seed events across a UTC-day boundary that does NOT coincide with the local cutoff. Verify each event uses the correct UTC key. Verify report-build for a business_day pulls events tagged with two different UTC keys.
- Simulate a 4-day-old event: rotation prunes its key, decrypt fails cleanly, daily_report row still readable.
- EOD pipeline: verify it no longer touches `dek`. Crypto-shred test that previously asserted post-EOD-undecryptability now fails — replace with the equivalent rotation-service test.

---

## Migration & rollout

This is a single-step migration in pre-prod. New migration file `0006_dek_rotation.sql`:
1. `DROP TABLE day_key;`
2. `CREATE TABLE dek (...);`
3. `event.key_id` column already exists; semantics change in code only.

Anyone running an older binary against a newer DB (or vice versa) breaks immediately by design — no graceful rollback. Acceptable in pre-prod.

---

## Threat model & SSD caveat

The "crypto-shred at T+3 days" guarantee assumes deleting a `dek` row from SQLite makes the wrapped DEK unrecoverable. **On SSD media this is not reliable at the SQLite layer.** Wear-leveling and the FTL relocate writes physically; freed cells linger until block erase + GC, on the controller's schedule (minutes to days, sometimes longer). `PRAGMA secure_delete` overwrites *logical* pages with zeros, but the original physical NAND cells often remain readable to a forensic attacker with raw disk access.

Realistic recovery against the current build, attacker who has the running cashier (or its disk + login password):

| Layer | Live data | Forensically-recovered "deleted" data |
|---|---|---|
| KEK in OS keychain | accessible (logged-in user) | n/a — never on app's disk |
| Wrapped DEKs in `master.dek` | last 4 days | older wrapped DEKs may persist in freed pages |
| Event ciphertext in `events.event` | last 4 business days | older ciphertext may persist in freed pages |
| Daily reports `reports/<day>.json` | **all of them, plaintext, forever** | n/a |

Attacker chains `(live KEK) + (recovered wrapped DEK) + (recovered ciphertext)` to decrypt old events. The 4-day window is the *honest* upper bound only when free pages are not recoverable.

**Mitigations that work on SSD:**

1. **Full-disk encryption (Bitlocker / FileVault) — required for any real protection.** Drive controller sees only ciphertext, so freed cells contain ciphertext that's useless without the disk key. Removes the entire raw-sector-scan attack class regardless of what SQLite does.
2. **Hardware-backed KEK (TPM-2 on Windows, Secure Enclave on macOS).** Prevents KEK extraction to a different machine even with disk + login password. Optional enhancement.
3. **Reports are plaintext** — the largest leak surface for historical data. Restrict file ACLs at minimum; consider a separate retention-key encryption in a future spec.

**Operational policy implied:**
- FDE is a deployment prerequisite. Document in install guide.
- Cashier process should run as non-admin user on Windows.
- OS-level auto-lock in addition to the app's idle lock.

`PRAGMA secure_delete` is intentionally NOT enabled — on SSD it gives false comfort without real protection, and on FDE-protected disks it's redundant. The hardening that matters lives below SQLite (the disk) and above it (the keychain).

---

## Out of scope

- **Tor / `arti` / `.onion` hidden service exposure.** If the original brief intended this (remote management without port forwarding), it warrants its own spec. Touchpoints: axum binding, Tauri sidecar process for arti, key publication. Wholly orthogonal to crypto rotation.
- **Re-encryption of existing data on rotation** (Cloak does support this). Not needed here — events are short-lived (3-day key TTL bounds them anyway).
- **Per-staff or per-spot key derivation.** Single venue-wide DEK per UTC day.
- **HSM / hardware-backed KEK.** KEK still lives in OS keychain.
- **Configurable retention.** 3 days is hard-coded. Settable later if needed.

---

## Open questions

1. **`.onion / arti`**: confirm whether this is in scope. If yes, separate spec.
2. **Event-row deletion at EOD**: include in this work, or split? Recommend including — it's the "clean up data based on cutoff" half of the brief.
3. **`GET /admin/keys` endpoint**: ship in v1 or defer? Recommend v1 (small, helps ops debug rotation).
