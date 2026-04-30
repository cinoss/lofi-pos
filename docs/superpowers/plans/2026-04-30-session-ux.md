# Session UX — Item Cancel/Return + Time-Billed Clock + Transfer

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Surface three pieces of session UX that the backend already supports but the cashier UI doesn't:

1. **Per-item cancel + return** in the session detail screen (today: place orders only; mistakes can't be undone in the UI)
2. **Time-billed room running clock** showing accumulated hourly charges in real time for room sessions (today: rooms have `hourly_rate` but the UI never shows the running total)
3. **Session transfer** to a different spot (today: customer wants to move from a table to a room — no UI for it)

Out of scope (separate plan): session merge/split — more complex UX (which items move where), best as its own design pass.

---

## Architecture & decisions

- **Cancel/return** are per-item actions inside session detail. Cancel is a binary toggle (set the whole line cancelled); return takes a quantity. Both go through ACL, with override-PIN flow when the actor lacks the role (Manager+ for `CancelOrderItemAny` / `ReturnOrderItem`; Cashier for self-cancel within grace window via `CancelOrderItemSelf`).
- **Running clock** lives in `session-detail.tsx`. For room sessions, compute `elapsed_ms = now_ms - session.opened_at_ms` in the browser, refresh once per minute. Time charge = `ceil(elapsed_min / 60) * hourly_rate` (charge by the hour, not by the minute, to match how Vietnamese karaoke bars typically bill — fractional hour rounded up). Show alongside the order subtotal: `Time: 02:14 — 200,000đ` plus `Items: 350,000đ` plus `Total now: 550,000đ`.
- **`opened_at_ms`** must be added to `SessionState` (Rust + zod) so the UI can compute elapsed. Currently it's recorded in the event but not surfaced in the state read API.
- **Transfer** is a button in session detail ("Move…") that opens a modal showing the spot picker (filtered to idle spots). Choose target → POST `/sessions/:id/transfer` with override flow.
- **Time-charge persistence**: the UI display is informational. The actual time charge is finalized at payment. Either:
  - (a) Auto-add a "Room time" line item when payment screen loads for a room session, OR
  - (b) Show in the payment screen as a separate `time_charge` field that gets included in subtotal.
  Recommend **(a)** — fits existing data model (everything is a line item) and prints cleanly on the receipt. The payment screen detects "room session, no time line item yet" and creates one via place-order with a synthetic `product` representing the room's hourly billing.
  
  However, this requires either a real product with `kind='time'` already in the catalog OR a dedicated endpoint. Existing schema has `Product.kind in ('item','recipe','time')` — `time` is the right tag. We'll require the venue to have a "Room time" product per room (or a single shared "Room hour") and the payment screen picks it. If absent, the time charge becomes informational-only with a warning ("set up a Time product to bill rooms").

  **Pragmatic v1:** show running clock + estimated charge in session-detail. At payment time, prefill the editable subtotal field with `items_subtotal + room_time_charge` so cashier sees the correct number; let the time-charge line item remain a future improvement. Keeps scope tight.

- **Order-item indices** in the `cancel` and `return` URLs are 0-based positions within an `OrderState.items` array. UI passes them through unchanged.
- **Override flow** — reuse the existing `<OverrideModal />` from `pos-ui`. Same pattern as payment.

---

## File Structure

```
apps/cashier/src-tauri/src/
  store/aggregate_store.rs              # MODIFIED — Session aggregate gains opened_at_ms
                                        # (already in event; just thread through to state)
  http/routes/session.rs                # MODIFIED — get_session response includes opened_at_ms

packages/shared/src/schemas/
  session.ts                            # MODIFIED — SessionState.opened_at_ms; ensure all server
                                        # responses include it (no migration needed; in-memory only)
  order.ts                              # MODIFIED — ensure CancelOrderItemInput +
                                        # ReturnOrderItemInput already exist (verify)

packages/pos-ui/src/routes/
  session-detail.tsx                    # MODIFIED — add cancel/return per-item buttons; render
                                        # running clock for room sessions; Transfer button
  payment.tsx                           # MODIFIED — prefill subtotal with computed items + time

packages/pos-ui/src/components/
  transfer-modal.tsx                    # NEW — spot picker modal for transfer
  room-clock.tsx                        # NEW — extracted reusable running-clock display
```

---

## Task 1: Expose `opened_at_ms` on SessionState

**Files:**
- Modify: `apps/cashier/src-tauri/src/store/aggregate_store.rs` (Session aggregate keeps opened_at_ms)
- Modify: `apps/cashier/src-tauri/src/http/routes/session.rs` (response serializes opened_at_ms)
- Modify: `packages/shared/src/schemas/session.ts`

- [ ] **Step 1: Rust Session aggregate**

In `aggregate_store.rs`, find the `Session` struct (or wherever the aggregate state shape lives). Add `pub opened_at_ms: i64`. The `apply` function for `SessionOpened` event already has the timestamp — assign it.

- [ ] **Step 2: HTTP response**

Whatever serde struct backs `GET /sessions/:id` now includes `opened_at_ms: i64`. Likely shared with `GET /sessions/active`.

- [ ] **Step 3: zod**

```ts
export const SessionState = z.object({
  session_id: z.string(),
  status: SessionStatus,
  spot: SpotRef,
  opened_by: z.number().int(),
  opened_at_ms: z.number().int(),  // NEW
  customer_label: z.string().nullable(),
  team: z.string().nullable(),
  order_ids: z.array(z.string()),
}).strict();
```

- [ ] **Step 4: tests**

Update existing session integration tests that snapshot `SessionState` — assertion gets a new field.

- [ ] **Step 5: cargo test + pnpm typecheck**

- [ ] **Step 6: Commit**

```bash
git add -A
git commit -m "feat(session): expose opened_at_ms on SessionState (for running clock UI)"
```

---

## Task 2: Per-item cancel + return UX

**Files:**
- Modify: `packages/pos-ui/src/routes/session-detail.tsx`

- [ ] **Step 1: verify shared schemas**

Confirm `CancelOrderItemInput` and `ReturnOrderItemInput` exist in `packages/shared/src/schemas/order.ts`. Likely shape:

```ts
export const CancelOrderItemInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  reason: z.string().nullable().optional(),
}).strict();

export const ReturnOrderItemInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  qty: z.number().int().positive(),
  reason: z.string().nullable().optional(),
}).strict();
```

If missing, add and re-export.

- [ ] **Step 2: cancel button per line**

In session-detail's order list rendering, alongside each item add buttons:

```tsx
{!it.cancelled && (
  <>
    <Button
      size="sm"
      variant="ghost"
      onClick={() => onCancelItem(o.order_id, i)}
    >
      <Trans>Cancel</Trans>
    </Button>
    <Button
      size="sm"
      variant="ghost"
      onClick={() => promptReturn(o.order_id, i, it.spec.qty - it.returned_qty)}
    >
      <Trans>Return</Trans>
    </Button>
  </>
)}
```

`onCancelItem` calls a new `cancelItem` mutation:

```tsx
const cancelItem = useMutation({
  mutationFn: async ({ orderId, idx, override }: {
    orderId: string;
    idx: number;
    override?: string;
  }) => {
    const input: CancelOrderItemInput = {
      idempotency_key: crypto.randomUUID(),
      ...(override ? { override_pin: override } : {}),
    };
    return apiClient.post(`/orders/${orderId}/items/${idx}/cancel`, OrderState, input);
  },
  onSuccess: () => qc.invalidateQueries({ queryKey: ["session", sessionId] }),
  onError: (e: unknown) => {
    if (e instanceof ApiError && e.code === "override_required") {
      setOverrideContext({ kind: "cancel", orderId, idx, role: e.envelope.message ?? "manager" });
    }
  },
});
```

- [ ] **Step 3: return modal**

Returning needs a quantity. Small inline modal (reuse existing modal pattern):
- Title: "Return how many?"
- Number input (max = remaining qty)
- Optional reason textarea
- Submit / cancel

On submit → `returnItem` mutation. Same override flow.

- [ ] **Step 4: override flow**

Single shared `OverrideModal` for cancel/return — when `override_required` ApiError fires, capture `{ kind, orderId, idx, qty? }` in state, show modal, retry mutation with `override_pin` from modal callback.

- [ ] **Step 5: i18n strings**

Wrap with `<Trans>`. Add to vi PO:
```
"Cancel" → "Hủy"
"Return" → "Trả hàng"
"Return how many?" → "Trả lại bao nhiêu?"
"Reason" → "Lý do"
"Submit" → "Gửi"
```

- [ ] **Step 6: tests**

Backend already has tests for cancel/return. UI smoke is manual.

- [ ] **Step 7: typecheck + build**

```bash
just cashier-i18n-extract && just cashier-i18n-compile
pnpm --filter @lofi-pos/cashier typecheck
pnpm --filter @lofi-pos/cashier build
```

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(pos-ui): per-item cancel + return with override flow"
```

---

## Task 3: Time-billed room running clock

**Files:**
- Create: `packages/pos-ui/src/components/room-clock.tsx`
- Modify: `packages/pos-ui/src/routes/session-detail.tsx`
- Modify: `packages/pos-ui/src/routes/payment.tsx`

- [ ] **Step 1: RoomClock component**

```tsx
// packages/pos-ui/src/components/room-clock.tsx
import { useEffect, useState } from "react";
import { Trans } from "@lingui/react/macro";

interface RoomClockProps {
  openedAtMs: number;
  hourlyRate: number;
  /** Refresh interval; default 30s */
  refreshMs?: number;
}

export function RoomClock({ openedAtMs, hourlyRate, refreshMs = 30_000 }: RoomClockProps) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), refreshMs);
    return () => clearInterval(t);
  }, [refreshMs]);

  const elapsedMs = Math.max(0, now - openedAtMs);
  const elapsedMin = Math.floor(elapsedMs / 60_000);
  const hh = Math.floor(elapsedMin / 60);
  const mm = elapsedMin % 60;

  // Bill by ceil(hours), rounded up to next full hour
  const billableHours = Math.max(1, Math.ceil(elapsedMin / 60));
  const charge = billableHours * hourlyRate;

  return (
    <div className="rounded border bg-blue-50 p-3 mb-2">
      <div className="text-xs text-gray-600">
        <Trans>Room time</Trans>
      </div>
      <div className="text-lg font-mono">
        {hh.toString().padStart(2, "0")}:{mm.toString().padStart(2, "0")}
      </div>
      <div className="text-sm text-gray-700">
        {billableHours}h × {hourlyRate.toLocaleString("vi-VN")}đ ={" "}
        <span className="font-semibold">{charge.toLocaleString("vi-VN")}đ</span>
      </div>
    </div>
  );
}

export function computeRoomCharge(openedAtMs: number, nowMs: number, hourlyRate: number): number {
  const elapsedMin = Math.max(0, Math.floor((nowMs - openedAtMs) / 60_000));
  const billableHours = Math.max(1, Math.ceil(elapsedMin / 60));
  return billableHours * hourlyRate;
}
```

Re-export from pos-ui index.

- [ ] **Step 2: render in session detail**

Where `session.spot.kind === "room"` and `session.status === "Open"`:

```tsx
{session.spot.kind === "room" && session.status === "Open" && (
  <RoomClock
    openedAtMs={session.opened_at_ms}
    hourlyRate={session.spot.hourly_rate ?? 0}
  />
)}
```

(SpotRef shape — confirm `hourly_rate` is included on the room variant. It is per the existing common schema. If undefined, treat as 0.)

- [ ] **Step 3: payment screen prefill**

When payment route loads:

```tsx
import { computeRoomCharge } from "../components/room-clock";

const roomCharge =
  session?.spot.kind === "room" && session.opened_at_ms
    ? computeRoomCharge(session.opened_at_ms, Date.now(), session.spot.hourly_rate ?? 0)
    : 0;

// existing computedSubtotal lives; default subtotal becomes:
useEffect(() => {
  if (subtotal === 0 && (computedSubtotal > 0 || roomCharge > 0)) {
    setSubtotal(computedSubtotal + roomCharge);
  }
}, [computedSubtotal, roomCharge]);
```

Display the room charge as a separate line in the breakdown so cashier sees how subtotal was assembled:

```tsx
{roomCharge > 0 && (
  <div className="text-sm text-gray-600">
    <Trans>Room time charge:</Trans> {roomCharge.toLocaleString("vi-VN")}đ
  </div>
)}
```

- [ ] **Step 4: tests**

Add a unit test for `computeRoomCharge`:
```tsx
describe("computeRoomCharge", () => {
  it("rounds up to full hour", () => {
    const start = 0;
    expect(computeRoomCharge(start, 5 * 60_000, 50_000)).toBe(50_000);  // 5min → 1h
    expect(computeRoomCharge(start, 65 * 60_000, 50_000)).toBe(100_000); // 65min → 2h
    expect(computeRoomCharge(start, 0, 50_000)).toBe(50_000); // floor 1h minimum
  });
});
```

(If pos-ui doesn't currently have a test runner setup, skip — manual smoke covers this.)

- [ ] **Step 5: i18n**

Add to PO:
```
"Room time" → "Thời gian phòng"
"Room time charge:" → "Tiền phòng:"
```

- [ ] **Step 6: typecheck + build**

- [ ] **Step 7: Commit**

```bash
git add -A
git commit -m "feat(pos-ui): room running clock + payment prefill (time-billed sessions)"
```

---

## Task 4: Session transfer

**Files:**
- Create: `packages/pos-ui/src/components/transfer-modal.tsx`
- Modify: `packages/pos-ui/src/routes/session-detail.tsx`
- Modify: `packages/shared/src/schemas/session.ts` (verify `TransferSessionInput` exists — it does)

- [ ] **Step 1: TransferModal**

```tsx
// packages/pos-ui/src/components/transfer-modal.tsx
import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { Trans } from "@lingui/react/macro";
import { Spot } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { useApiClient } from "../api-context";

interface TransferModalProps {
  currentSpotId: number;
  onClose: () => void;
  onSelect: (spotId: number) => void;
}

export function TransferModal({ currentSpotId, onClose, onSelect }: TransferModalProps) {
  const apiClient = useApiClient();
  const { data: spots } = useQuery({
    queryKey: ["spots"],
    queryFn: () => apiClient.get("/spots", z.array(Spot)),
  });

  const candidates = (spots ?? [])
    .filter((s) => s.id !== currentSpotId && s.status === "idle");

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white rounded p-6 max-w-md w-full">
        <h2 className="text-xl font-semibold mb-3"><Trans>Move to spot</Trans></h2>
        {candidates.length === 0 ? (
          <p className="text-gray-500 text-sm"><Trans>No idle spots available.</Trans></p>
        ) : (
          <ul className="grid grid-cols-2 gap-2 max-h-80 overflow-auto">
            {candidates.map((s) => (
              <li key={s.id}>
                <Button
                  variant="outline"
                  className="w-full text-left"
                  onClick={() => onSelect(s.id)}
                >
                  {s.name} <span className="text-xs text-gray-500">({s.kind})</span>
                </Button>
              </li>
            ))}
          </ul>
        )}
        <div className="mt-4 text-right">
          <Button variant="ghost" onClick={onClose}><Trans>Cancel</Trans></Button>
        </div>
      </div>
    </div>
  );
}
```

- [ ] **Step 2: wire in session detail**

```tsx
const [showTransfer, setShowTransfer] = useState(false);
const [transferTarget, setTransferTarget] = useState<{ spotId: number; override?: string } | null>(null);

const transfer = useMutation({
  mutationFn: ({ spotId, override }: { spotId: number; override?: string }) => {
    const input: TransferSessionInput = {
      idempotency_key: crypto.randomUUID(),
      to_spot_id: spotId,
      ...(override ? { override_pin: override } : {}),
    };
    return apiClient.post(`/sessions/${sessionId}/transfer`, SessionState, input);
  },
  onSuccess: () => {
    qc.invalidateQueries({ queryKey: ["session", sessionId] });
    qc.invalidateQueries({ queryKey: ["sessions", "active"] });
    setShowTransfer(false);
    setTransferTarget(null);
  },
  onError: (e) => {
    if (e instanceof ApiError && e.code === "override_required") {
      setOverrideContext({ kind: "transfer", spotId: transferTarget!.spotId, role: e.envelope.message ?? "manager" });
    }
  },
});

// Button in actions row:
<Button variant="outline" onClick={() => setShowTransfer(true)}><Trans>Move…</Trans></Button>

// At route bottom:
{showTransfer && (
  <TransferModal
    currentSpotId={session.spot.id}
    onClose={() => setShowTransfer(false)}
    onSelect={(spotId) => {
      setTransferTarget({ spotId });
      transfer.mutate({ spotId });
    }}
  />
)}
```

- [ ] **Step 3: i18n**

```
"Move…" → "Chuyển..."
"Move to spot" → "Chuyển sang bàn / phòng"
"No idle spots available." → "Không có bàn / phòng trống."
```

- [ ] **Step 4: typecheck + build**

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pos-ui): session transfer — Move… modal with spot picker + override flow"
```

---

## Task 5: Final gate

- [ ] `cd apps/cashier/src-tauri && cargo test` — should grow by 1-2 (opened_at_ms field test)
- [ ] `cargo clippy --all-targets -- -D warnings` clean
- [ ] All workspace typechecks: shared, pos-ui, cashier, web, admin
- [ ] `just i18n-extract && just i18n-compile` clean (catalogs sync)
- [ ] `pnpm --filter @lofi-pos/cashier build` succeeds

Manual smoke (recommended for this UX-heavy plan):
- Open a room session, verify clock starts at 00:00
- Wait 5 minutes, verify clock shows 00:05 and charge = 1h × room rate
- Place items in a session, cancel one — UI updates
- Return 1 of 3 of an item — UI shows `(1 returned)`
- Click Move…, choose target spot, confirm transfer; verify session shows new spot

---

## Done

End state:
- `SessionState.opened_at_ms` exposed on the wire (Rust + zod)
- Session detail shows cancel + return per item with override flow
- Room sessions render a running clock + computed time charge updating every 30s
- Payment screen prefills subtotal with `items + time_charge` for room sessions
- Transfer button + modal lets staff move a session to any idle spot

Out of scope, tracked for follow-up:
- Session merge/split UX (item-by-item picker is the hard part)
- Time-charge as a real line item (`product.kind='time'` integration) — current behavior is informational/prefill only
- Refund flow beyond return (e.g., comping a closed session)
- Per-item discount UX (currently only session-level discount %)
