# Room Billing Config — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the room `hourly_rate` integer with a JSON `billing_config` blob covering hourly rate, bucket size (minutes), included duration, and minimum charge. Snapshot the config into `SpotRef::Room` at session-open / transfer time so historical sessions bill against the policy that was in effect.

**Pricing semantics:**
```
elapsed_min ≤ included_minutes  → charge = min_charge
elapsed_min  > included_minutes  →
    overage_min     = elapsed_min - included_minutes
    overage_buckets = ceil(overage_min / bucket_minutes)
    overage_charge  = ceil(overage_buckets * bucket_minutes * hourly_rate / 60)
    charge          = min_charge + overage_charge
```

Defaults for a new spot: `{ hourly_rate, bucket_minutes: 1, included_minutes: 0, min_charge: 0 }` — pure per-minute billing.

To match TODAY's "ceil-to-hour, 1-hour minimum" behavior: `{ bucket_minutes: 60, included_minutes: 60, min_charge: hourly_rate }`. (Migration does NOT auto-pick this; new field defaults give per-minute. Owner can dial in the old behavior via the admin form.)

**Pre-prod migration**: drop+recreate `spot` with `hourly_rate` removed, replaced by `billing_config TEXT NULL`. Existing dev data lost — acceptable.

---

## File Structure

```
apps/cashier/src-tauri/src/
  domain/spot.rs                                # MODIFIED — RoomBilling struct;
                                                # SpotRef::Room { id, name, billing }
  store/migrations/master/
    0010_spot_billing_config.sql                # NEW — drop+recreate spot;
                                                # billing_config TEXT NULL
  store/master.rs                               # MODIFIED — Spot row I/O
                                                # serializes billing JSON
  http/routes/admin.rs                          # MODIFIED — SpotInput accepts
                                                # billing_config object for rooms
  http/routes/session.rs                        # MODIFIED — open_session loads
                                                # spot, builds SpotRef::Room with
                                                # billing snapshot

packages/shared/src/schemas/
  common.ts                                     # MODIFIED — SpotRef.Room.billing
  admin.ts                                      # MODIFIED — SpotInput.billing_config

packages/pos-ui/src/components/room-clock.tsx   # MODIFIED — accepts RoomBilling;
                                                # computeRoomCharge applies all 4 fields;
                                                # display HH:MM:SS w/ 1s refresh
packages/pos-ui/src/routes/payment.tsx          # MODIFIED — pass session.spot.billing
                                                # into computeRoomCharge

apps/admin/src/routes/spots.tsx                 # MODIFIED — billing fields in form
                                                # when kind=room (4 numeric inputs)
```

Decisions:
- **billing_config is `TEXT` not SQLite JSON1 type** — sqlite JSON support varies by build; storing as TEXT and parsing in code is portable.
- **Only rooms have billing.** Tables get `billing_config = NULL`. SpotRef::Table unchanged.
- **JSON shape on disk + on wire is identical**: `{ hourly_rate, bucket_minutes, included_minutes, min_charge }`. All four are required when present (no per-field defaults at the JSON layer; they're populated by the admin form).
- **No backward compat for the old `hourly_rate` column** — pre-prod, drop and recreate.
- **Snapshot lives in event payload via SpotRef** — adding a new nested object inside SpotRef::Room is a serde-compatible extension since SpotRef is JSON-serialized inside the encrypted blob; old events still decode (no `billing` field → serde default → empty/None handling). For pre-prod we don't need to handle this; new events all carry billing. But add `#[serde(default)]` for safety.

---

## Task 1: TDD computeRoomCharge

**Files:**
- Modify: `packages/pos-ui/src/components/room-clock.tsx`

- [ ] **Step 1: tests first** (write before implementation)

If pos-ui doesn't have a test runner, add Vitest:

```bash
pnpm --filter @lofi-pos/pos-ui add -D vitest
```

Add `packages/pos-ui/src/components/room-clock.test.ts`:

```ts
import { describe, it, expect } from "vitest";
import { computeRoomCharge } from "./room-clock";

const MIN = 60_000;
const billing = (h: number, b: number, inc: number, min: number) => ({
  hourly_rate: h,
  bucket_minutes: b,
  included_minutes: inc,
  min_charge: min,
});

describe("computeRoomCharge", () => {
  it("zero elapsed → 0 when no minimum", () => {
    expect(computeRoomCharge(0, 0, billing(150_000, 1, 0, 0))).toBe(0);
  });

  it("per-minute, no minimum: 5 min @ 150k/h → 12,500", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(150_000, 1, 0, 0))).toBe(12_500);
  });

  it("per-minute, no minimum: 65 min @ 150k/h → 162,500", () => {
    expect(computeRoomCharge(0, 65 * MIN, billing(150_000, 1, 0, 0))).toBe(162_500);
  });

  it("rounds up sub-minute partial: 90 sec @ 60k/h → 1500", () => {
    // 90s with bucket=1min → 2 buckets → ceil(2 * 1 * 60000 / 60) = 2000
    expect(computeRoomCharge(0, 90_000, billing(60_000, 1, 0, 0))).toBe(2000);
  });

  it("15-min bucket: 5 min @ 60k/h → 15,000 (1 bucket)", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(60_000, 15, 0, 0))).toBe(15_000);
  });

  it("15-min bucket: 16 min @ 60k/h → 30,000 (2 buckets)", () => {
    expect(computeRoomCharge(0, 16 * MIN, billing(60_000, 15, 0, 0))).toBe(30_000);
  });

  it("hour bucket + included = matches old behavior: 5 min @ 150k/h, bucket=60, included=60, min=150k → 150k", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(150_000, 60, 60, 150_000))).toBe(150_000);
  });

  it("hour bucket + included: 65 min @ 150k/h, bucket=60, included=60, min=150k → 300k", () => {
    expect(computeRoomCharge(0, 65 * MIN, billing(150_000, 60, 60, 150_000))).toBe(300_000);
  });

  it("min charge floor: 5 min @ 60k/h, included=30, min=20k → 20k", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(60_000, 1, 30, 20_000))).toBe(20_000);
  });

  it("included covers exactly: 30 min, included=30, min=20k → 20k", () => {
    expect(computeRoomCharge(0, 30 * MIN, billing(60_000, 1, 30, 20_000))).toBe(20_000);
  });

  it("over-included by 1 min: 31 min, included=30, min=20k → 20k + 1 min @ 60k/h = 21k", () => {
    expect(computeRoomCharge(0, 31 * MIN, billing(60_000, 1, 30, 20_000))).toBe(21_000);
  });

  it("over-included by 5 min, 5-min bucket: 35 min, included=30, bucket=5, min=20k → 20k + 1 bucket * 5 * 60k / 60 = 25k", () => {
    expect(computeRoomCharge(0, 35 * MIN, billing(60_000, 5, 30, 20_000))).toBe(25_000);
  });

  it("over-included by 6 min, 5-min bucket: 36 min → ceil(6/5)=2 buckets = 30k overage → 50k total", () => {
    expect(computeRoomCharge(0, 36 * MIN, billing(60_000, 5, 30, 20_000))).toBe(50_000);
  });
});
```

Add `vitest` script to `packages/pos-ui/package.json`:
```json
"test": "vitest run"
```

- [ ] **Step 2: implement**

```ts
// packages/pos-ui/src/components/room-clock.tsx
export interface RoomBilling {
  hourly_rate: number;       // VND per hour
  bucket_minutes: number;    // billing granularity
  included_minutes: number;  // covered by min_charge
  min_charge: number;        // VND minimum
}

export function computeRoomCharge(
  openedAtMs: number,
  nowMs: number,
  billing: RoomBilling,
): number {
  const elapsedMs = Math.max(0, nowMs - openedAtMs);
  const elapsedMin = Math.floor(elapsedMs / 60_000);
  if (elapsedMin <= billing.included_minutes) {
    return billing.min_charge;
  }
  const overageMin = elapsedMin - billing.included_minutes;
  const overageBuckets = Math.ceil(overageMin / billing.bucket_minutes);
  const overageCharge = Math.ceil(
    (overageBuckets * billing.bucket_minutes * billing.hourly_rate) / 60,
  );
  return billing.min_charge + overageCharge;
}
```

- [ ] **Step 3: run vitest — all tests should pass**

```bash
pnpm --filter @lofi-pos/pos-ui test
```

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(pos-ui): RoomBilling type + computeRoomCharge w/ bucket+included+min"
```

---

## Task 2: Backend schema + RoomBilling

**Files:**
- Create: `apps/cashier/src-tauri/src/store/migrations/master/0010_spot_billing_config.sql`
- Modify: `apps/cashier/src-tauri/src/domain/spot.rs`
- Modify: `apps/cashier/src-tauri/src/store/master.rs`

- [ ] **Step 1: migration**

```sql
-- Pre-prod destructive migration: spot now carries a JSON billing_config.
DROP TABLE IF EXISTS spot;
CREATE TABLE spot (
  id              INTEGER PRIMARY KEY AUTOINCREMENT,
  name            TEXT NOT NULL,
  kind            TEXT NOT NULL CHECK (kind IN ('room','table')),
  parent_id       INTEGER REFERENCES spot(id) ON DELETE SET NULL,
  status          TEXT NOT NULL DEFAULT 'idle',
  -- Room billing policy as JSON: {hourly_rate, bucket_minutes,
  -- included_minutes, min_charge}. NULL for tables. Snapshotted into
  -- SpotRef::Room at session-open/transfer time.
  billing_config  TEXT
);
CREATE INDEX idx_spot_kind ON spot(kind);
```

- [ ] **Step 2: Rust types**

In `domain/spot.rs`:
```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoomBilling {
    pub hourly_rate: i64,
    pub bucket_minutes: u32,
    pub included_minutes: u32,
    pub min_charge: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum SpotRef {
    Room {
        id: i64,
        name: String,
        billing: RoomBilling,
    },
    Table {
        id: i64,
        name: String,
    },
}
```

(Existing `id()`, `name()`, `is_room()`, `is_table()` impls stay.)

- [ ] **Step 3: Master CRUD**

`master.rs` Spot insert/update/list now serializes `billing_config: Option<String>` (JSON). Provide convenience helpers `Spot::billing()` returning `Option<RoomBilling>` for in-Rust callers.

Tests: round-trip a room w/ billing JSON; round-trip a table with NULL billing.

- [ ] **Step 4: cargo test**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(spot): JSON billing_config; SpotRef::Room captures RoomBilling"
```

---

## Task 3: HTTP + zod schemas

**Files:**
- Modify: `apps/cashier/src-tauri/src/http/routes/admin.rs`
- Modify: `apps/cashier/src-tauri/src/http/routes/session.rs`
- Modify: `packages/shared/src/schemas/common.ts`
- Modify: `packages/shared/src/schemas/admin.ts`

- [ ] **Step 1: SpotInput accepts billing**

```rust
// admin.rs SpotInput
#[derive(Deserialize)]
pub struct SpotInput {
    pub name: String,
    pub kind: String,
    pub parent_id: Option<i64>,
    pub billing_config: Option<RoomBilling>,
}
```

Validation: if `kind == "room"` then `billing_config` must be `Some` with `hourly_rate >= 0`, `bucket_minutes >= 1`, `included_minutes >= 0`, `min_charge >= 0`. If `kind == "table"`, ignore (force NULL).

- [ ] **Step 2: open_session captures billing**

When `open_session` constructs `SpotRef::Room`, fetch the spot's `billing_config` from master and embed it. Same for SessionTransferred destination.

- [ ] **Step 3: zod**

```ts
// packages/shared/src/schemas/common.ts
export const RoomBilling = z.object({
  hourly_rate: z.number().int().nonnegative(),
  bucket_minutes: z.number().int().positive(),
  included_minutes: z.number().int().nonnegative(),
  min_charge: z.number().int().nonnegative(),
}).strict();

export const SpotRef = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("room"),
    id: z.number().int(),
    name: z.string(),
    billing: RoomBilling,
  }).strict(),
  z.object({
    kind: z.literal("table"),
    id: z.number().int(),
    name: z.string(),
  }).strict(),
]);
```

```ts
// packages/shared/src/schemas/admin.ts
export const SpotInput = z.object({
  name: z.string().min(1),
  kind: z.enum(["room", "table"]),
  parent_id: z.number().int().nullable(),
  billing_config: RoomBilling.nullable(),
}).strict();
```

The existing flat `hourly_rate` field is removed from SpotInput.

- [ ] **Step 4: integration tests**

```rust
// admin spots: create room w/ billing → list returns same; create table → billing null
// session: open on room → returned SessionState.spot.billing matches the spot's config
```

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(http): admin SpotInput accepts billing_config; open_session snapshots it"
```

---

## Task 4: RoomClock UI + payment + admin form

**Files:**
- Modify: `packages/pos-ui/src/components/room-clock.tsx` (display)
- Modify: `packages/pos-ui/src/routes/payment.tsx`
- Modify: `apps/admin/src/routes/spots.tsx`

- [ ] **Step 1: RoomClock display**

Take `billing: RoomBilling` instead of `hourlyRate: number`. Refresh = 1s. Display `HH:MM:SS`. Show formula breakdown:

```tsx
<div className="text-sm text-gray-700">
  {billing.included_minutes > 0 && elapsedMin <= billing.included_minutes && (
    <Trans>Included period: {billing.min_charge.toLocaleString("vi-VN")}đ</Trans>
  )}
  {elapsedMin > billing.included_minutes && (
    <>
      {billing.min_charge > 0 && <>{billing.min_charge.toLocaleString("vi-VN")}đ + </>}
      {overageBuckets} × {billing.bucket_minutes}m × {billing.hourly_rate.toLocaleString("vi-VN")}đ/h
      {" = "}
      <span className="font-semibold">{charge.toLocaleString("vi-VN")}đ</span>
    </>
  )}
</div>
```

- [ ] **Step 2: payment.tsx**

```tsx
const roomCharge =
  session?.spot.kind === "room" && session.opened_at_ms
    ? computeRoomCharge(session.opened_at_ms, Date.now(), session.spot.billing)
    : 0;
```

- [ ] **Step 3: admin spots form**

When `kind === "room"`, render four numeric fields:
- Hourly rate (VND/h) — required positive
- Bucket (minutes) — default 1, min 1
- Included minutes — default 0, min 0
- Minimum charge (VND) — default 0, min 0

On submit, build `billing_config: { hourly_rate, bucket_minutes, included_minutes, min_charge }`. Hide for kind=table.

- [ ] **Step 4: typecheck + builds**

```bash
pnpm -r typecheck
pnpm -r build
```

- [ ] **Step 5: i18n**

Extract + translate new strings (Included period, etc.).

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(ui): RoomClock + payment use RoomBilling; admin form fields"
```

---

## Task 5: Final gate

- [ ] `cargo test` — should grow by ~3-4 (room billing round-trip + open_session snapshot)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] `pnpm -r typecheck` clean; `pnpm --filter @lofi-pos/pos-ui test` green
- [ ] `pnpm -r build` succeeds
- [ ] Manual smoke: edit a room's billing in admin, open session, watch clock tick to the second, take payment with prefilled correct amount

---

## Done

End state:
- Spot table carries `billing_config TEXT` JSON column (rooms only)
- `SpotRef::Room { id, name, billing: RoomBilling }` with snapshot at session-open
- `computeRoomCharge(openedAtMs, nowMs, billing)` applies bucket + included + min in a single pure function
- `RoomClock` ticks every second, shows breakdown
- Admin spot form gains four billing fields when kind=room
- Default for new rooms: per-minute, no minimum, no included period

Out of scope:
- Per-staff or per-day rate tiers (happy hour, weekend rate)
- Time-charge as a real `product.kind='time'` line item (still informational/prefill)
- Scheduled rate changes
