# Session Merge / Split — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Spec:** `docs/superpowers/specs/2026-05-04-session-merge-split-design.md`

**Goal:** Wire UI for merge (`A → B with A's room time as a Time line item`) and split (`A → A + new session N on a different spot, fresh hour, with selected items moved`).

**Backend:** Merge / split / open-session / place-order / cancel-item endpoints all exist. New: setup wizard seeds a "Room Time" product if none exists; that's it on the backend.

---

## Task 1: Setup wizard seeds a Room Time product

**Files:**
- Modify: `apps/cashier/src-tauri/src/http/routes/setup.rs`

- [ ] **Step 1: insertion alongside existing settings/owner inserts**

In `submit` handler's transaction, after the staff insert and before commit, INSERT OR IGNORE a Room Time product:

```rust
tx.execute(
    "INSERT OR IGNORE INTO product (name, price, route, kind) \
     VALUES ('Room Time', 0, 'none', 'time')",
    [],
)?;
```

(Using `INSERT OR IGNORE` is defensive — if the table already has a kind=time product on a re-runnable setup-already-complete path, we don't duplicate.)

- [ ] **Step 2: integration test**

Update an existing setup integration test (or add one): after `POST /admin/setup`, list products via `/products` and assert at least one has `kind=time`.

- [ ] **Step 3: cargo test setup_integration**

- [ ] **Step 4: Commit**

```bash
git add -A
git commit -m "feat(setup): seed a 'Room Time' product (kind=time) on first-run"
```

---

## Task 2: MergeModal + wire into session-detail

**Files:**
- Create: `packages/pos-ui/src/components/merge-modal.tsx`
- Modify: `packages/pos-ui/src/index.ts` (re-export)
- Modify: `packages/pos-ui/src/routes/session-detail.tsx`

- [ ] **Step 1: MergeModal component**

```tsx
// packages/pos-ui/src/components/merge-modal.tsx
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { Trans } from "@lingui/react/macro";
import { SessionState, Product } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { useApiClient } from "../api-context";
import { computeRoomCharge } from "./room-clock";

interface MergeModalProps {
  source: SessionState;
  onClose: () => void;
  onConfirm: (target: SessionState, timeChargeProductId: number, timeCharge: number) => Promise<void>;
  busy?: boolean;
}

export function MergeModal({ source, onClose, onConfirm, busy }: MergeModalProps) {
  const apiClient = useApiClient();
  const { data: actives } = useQuery({
    queryKey: ["sessions", "active"],
    queryFn: () => apiClient.get("/sessions/active", z.array(SessionState)),
  });
  const { data: products } = useQuery({
    queryKey: ["products"],
    queryFn: () => apiClient.get("/products", z.array(Product)),
  });

  const targets = (actives ?? []).filter(
    (s) => s.session_id !== source.session_id && s.status === "Open",
  );
  const timeProduct = (products ?? []).find((p) => p.kind === "time");
  const sourceTimeCharge =
    source.spot.kind === "room"
      ? computeRoomCharge(source.opened_at_ms, Date.now(), source.spot.billing)
      : 0;

  const [picked, setPicked] = useState<SessionState | null>(null);

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-6 max-w-lg w-full">
        <h2 className="text-xl font-semibold mb-3"><Trans>Merge into…</Trans></h2>
        {!picked ? (
          <>
            {targets.length === 0 ? (
              <p className="text-gray-500 text-sm"><Trans>No other open sessions.</Trans></p>
            ) : (
              <ul className="grid grid-cols-2 gap-2 max-h-80 overflow-auto">
                {targets.map((t) => (
                  <li key={t.session_id}>
                    <Button
                      variant="outline"
                      className="w-full h-16 flex flex-col"
                      onClick={() => setPicked(t)}
                    >
                      <span className="font-semibold">{t.spot.name}</span>
                      <span className="text-xs text-gray-500">{t.spot.kind}</span>
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            <div className="mt-4 text-right">
              <Button variant="ghost" onClick={onClose}><Trans>Cancel</Trans></Button>
            </div>
          </>
        ) : (
          <>
            <div className="mb-3 text-sm">
              <Trans>
                Move {source.order_ids.length} order(s) and {sourceTimeCharge.toLocaleString("vi-VN")}đ room time from <b>{source.spot.name}</b> into <b>{picked.spot.name}</b>.
              </Trans>
            </div>
            {sourceTimeCharge > 0 && !timeProduct && (
              <div className="text-red-600 text-sm mb-3">
                <Trans>No "Room Time" product configured. Run setup or add a product with kind=time.</Trans>
              </div>
            )}
            <div className="flex gap-2 justify-end">
              <Button variant="outline" onClick={() => setPicked(null)} disabled={busy}>
                <Trans>Back</Trans>
              </Button>
              <Button
                onClick={() => {
                  if (sourceTimeCharge > 0 && !timeProduct) return;
                  void onConfirm(picked, timeProduct?.id ?? 0, sourceTimeCharge);
                }}
                disabled={busy || (sourceTimeCharge > 0 && !timeProduct)}
              >
                <Trans>Confirm merge</Trans>
              </Button>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
```

- [ ] **Step 2: re-export from pos-ui index.ts**

- [ ] **Step 3: wire into session-detail.tsx**

Add a "Merge into…" button next to "Move…" (only when status=Open). On click → open MergeModal. On confirm → run the two-step:
- (a) place an order on target with synthesized items: source's currently-unmoved items (for each order in source.order_ids, fetch + map item.spec for non-cancelled items; subtract returned_qty) + a Room Time line item using the time-product id and computed charge
- (b) `POST /sessions/merge` with `{into_session: target.id, sources: [source.session_id]}`

Both use the existing `useMutation` + override flow pattern. On error of step (a), surface inline. On error of step (b) after step (a) succeeded, surface a recovery banner.

After success: `nav('/sessions/' + target.session_id)`.

- [ ] **Step 4: i18n**

Extract + translate strings (Merge into…, Confirm merge, No other open sessions., No "Room Time" product configured., Back, the {N} order(s) message).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pos-ui): MergeModal — fold source's items + room-time charge into target"
```

---

## Task 3: SplitModal + wire into session-detail

**Files:**
- Create: `packages/pos-ui/src/components/split-modal.tsx`
- Modify: `packages/pos-ui/src/index.ts`
- Modify: `packages/pos-ui/src/routes/session-detail.tsx`

- [ ] **Step 1: SplitModal component**

```tsx
// packages/pos-ui/src/components/split-modal.tsx
//
// Two sections in one modal:
//   1) Spot picker (idle only, current spot excluded). Reuses
//      activeSessions cross-reference like SpotPicker.
//   2) Item picker: list source's current line items (across all orders)
//      with checkbox + numeric qty input (default = remaining qty,
//      max-cap = remaining qty).
//
// Confirm button enabled only when: spotId set AND >=1 item with qty>0.
//
// Returns to caller via onConfirm({ spotId, items: [{orderId, idx, qty, fullCancel}] })
```

(Implement against the same patterns as TransferModal + ReturnPromptModal. Item rows mirror session-detail's existing rendering.)

- [ ] **Step 2: re-export**

- [ ] **Step 3: wire into session-detail.tsx**

Add "Split…" button next to Merge into… and Move… (status=Open only).

On confirm:
- Open new session N on chosen spot via `POST /sessions`
- For each item:
  - If qty == remaining → `POST /orders/:order_id/items/:idx/cancel`
  - Else (partial) → `POST /orders/:order_id/items/:idx/return` with chosen qty (return reduces returned_qty; this is the "remove N from this line" semantic)
- Place a single new order on N with the moved items as fresh OrderItemSpecs
- After success: `nav('/sessions/' + N.session_id)`

Failure surfaces inline with which step failed.

- [ ] **Step 4: i18n**

Extract + translate (Split…, Move which items?, full cancel vs partial return).

- [ ] **Step 5: Commit**

```bash
git add -A
git commit -m "feat(pos-ui): SplitModal — fork items into a fresh session on a new spot"
```

---

## Task 4: Final gate

- [ ] cargo test (was 269 after billing config + history) — should grow by 1-2 (setup time-product test)
- [ ] cargo clippy --all-targets -- -D warnings clean
- [ ] pnpm -r typecheck clean
- [ ] pnpm -r build clean
- [ ] pnpm --filter @lofi-pos/pos-ui test (existing 18 vitest cases) clean
- [ ] Manual smoke (recommended for this UX-heavy change):
  - Open A on Room 1 with hourly_rate; place 2 orders
  - Open B on Room 2 with hourly_rate; place 1 order
  - From A: Merge into B → confirm → A vanishes from active, B has A's orders + Time on Room 1
  - Open new C on Room 3 with hourly_rate; place 4 orders
  - From C: Split → pick Room 1 (now free) + select 2 items → confirm
  - C continues with 2 items, new session D on Room 1 has the 2 moved items + fresh clock at 0:00

---

## Done

End state:
- Cashier session-detail page has three side-by-side action buttons when Open: Move… (transfer), Merge into…, Split…
- Merge: source's room-time becomes a Time line item on target via the seeded "Room Time" product (kind=time)
- Split: new session opens on chosen spot with fresh hour; selected items moved via cancel + place-order
- Backend touched only at setup (seed time-product); merge/split/open/place-order/cancel routes already in place
