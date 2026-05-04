# Session Merge / Split — Design

**Status:** Active.

**Goal:** UI for two backend operations whose APIs already exist but have no cashier surface today. Both compose existing primitives (place-order, cancel-item, open-session, merge, split) — the new work is mostly UX + a tiny catalog seed.

## Semantics (per the operator)

**Merge** — close the source session, fold its items + room-time charge into the target session.

- Source `A` and target `B` are both Open.
- Source's room-time-so-far is computed via `computeRoomCharge(A.opened_at_ms, now, A.spot.billing)` and added as a single "Time on <A.spot.name>" line item on target `B`.
- Source's existing line items are also added to `B` (synthesized as a new Order on `B` so the items appear under `B`).
- After merge: `A` becomes `Merged{into: B}` and removed from active store; `B.order_ids` includes `A`'s original orders (audit) plus the new synthesized order.

**Split** — move some items from source `A` to a brand-new session `N` on a different spot; `N`'s room clock starts fresh.

- Operator picks a destination spot (idle) and which items to move (with optional partial qty).
- New session `N` is opened on that spot (fresh `opened_at_ms`).
- Selected items are cancelled on `A`'s orders (or partial-returned for partial-qty) and re-placed as a fresh order on `N`.
- `A` remains Open with the un-moved items.

## Backend changes

None to the merge/split commands themselves — the existing routes are sufficient. Two small additions:

1. **First-run setup seeds a "Room Time" product** with `kind='time'` if one doesn't exist. The synthesized merge line item needs a real `product_id`; using `kind='time'` semantically marks it as a room-time charge for future EOD reports.
2. (Optional) `GET /products?kind=time` filter for the frontend to find the time-product id without scanning the full list. Decide at impl time; the frontend can also filter client-side from the existing `/products` response.

## Frontend additions

### `MergeModal` (new, in `packages/pos-ui`)

- Fetches `/sessions/active`; displays other active sessions as tiles
- For the source session A, computes its current room-time charge inline
- On select target B: shows a one-screen preview
  - "Move N items + <charge>đ Time charge from A → B"
  - "After merge, A's room becomes available again."
  - Confirm / Cancel
- On confirm:
  - `POST /orders` to B with synthesized items (source's currently-unmoved items + Room Time line)
  - `POST /sessions/merge` with `{into_session: B, sources: [A]}`
  - Either step's failure leaves the system in a recoverable state (no partial commit hiding)
- Wired from session-detail "Merge into…" button (Open status only)

### `SplitModal` (new, in `packages/pos-ui`)

- Two-section layout:
  - **Top**: spot picker (idle, non-current) — reuses the same `/sessions/active` cross-reference as `SpotPicker`
  - **Bottom**: line-item picker for source A's orders — checkbox per item + qty input (default = remaining qty, max-cap)
- Confirm: requires at least one item selected and a spot chosen
- On confirm:
  - `POST /sessions` on chosen spot → fresh session N
  - For each selected item: `POST /orders/:order_id/items/:idx/cancel` (full) or `POST /orders/:order_id/items/:idx/return` with chosen qty (partial)
  - `POST /orders` on N with the moved items (fresh OrderItemSpec)
- Wired from session-detail "Split…" button (Open status only)
- After success: navigate to `/sessions/N`

### Common UX patterns

- Both modals use the existing `OverrideModal` for ACL override-PIN flow (same pattern as cancel/return)
- Both modals show inline error messages on each step's failure with a "Retry" or "Cancel" affordance

## Failure modes (acknowledged)

- **Merge step a fails (place order on B)**: nothing applied; user retries.
- **Merge step b fails after step a succeeded**: B has the new synthesized order; A is still Open. Banner: "Merge half-completed — cancel the new order on B and retry, OR retry merge."
- **Split step a fails (open N)**: nothing applied.
- **Split step b partial failure (some cancels succeed, others fail)**: A has some items cancelled, no new session yet. Banner: "Some items already removed from A; spot N not opened. Manually re-place them on A or retry."
- **Split step c failure (new order placement on N)**: A is missing items, N is open but empty. Banner: "Items removed from A but new session is empty. Place them manually on N."

These are small surfaces for two-step UX. A future "merge command that does it all atomically server-side" would close them; out of scope.

## Out of scope

- Multi-source merge in one operation (`sources: Vec<>` is supported by backend; UI picks one source at a time)
- History page showing merged source sessions (current apply() removes source from active store; events.db retains the SessionMerged event for EOD audit)
- Reassigning items between two existing sessions without closing either (partial-merge)
- Atomic backend command that wraps the multi-step UI flow (would need new event types or a "merge-command" service)

## Validation already in place

- Merge: target Open + sources Open + non-empty + distinct + target not in sources (validation_integration tests)
- Split target spot must be unoccupied (SessionOpened spot-occupancy check)
- Cancel/Return: standard guards (in-bounds, not-already-cancelled, qty ≤ remaining)

## Ordering / mutex

- Cashier's `CommandService.execute` already serializes per-aggregate. Multi-step UI flows are NOT in one transaction, but each step is individually safe and validated. Failure modes above are accepted.
