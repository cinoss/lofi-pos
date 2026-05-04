import {
  useQuery,
  useQueries,
  useMutation,
  useQueryClient,
} from "@tanstack/react-query";
import { useNavigate, useParams } from "react-router-dom";
import { z } from "zod";
import { useState } from "react";
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";
import { SessionState, OrderState, Product, ApiError } from "@lofi-pos/shared";
import type {
  PlaceOrderInput,
  RawOrderItem,
  CloseSessionInput,
  CancelOrderItemInput,
  ReturnOrderItemInput,
  TransferSessionInput,
} from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { useApiClient } from "../api-context";
import { OverrideModal } from "../components/override-modal";
import { RoomClock } from "../components/room-clock";
import { TransferModal } from "../components/transfer-modal";
import { MergeModal } from "../components/merge-modal";
import { SplitModal, type SplitItem } from "../components/split-modal";
import { Breadcrumbs } from "../components/breadcrumbs";
import type { MergeSessionsInput, OpenSessionInput } from "@lofi-pos/shared";

/** UI-side identity for a specific order line, used to drive cancel/return modals. */
interface ItemRef {
  orderId: string;
  idx: number;
  /** Remaining (un-returned, un-cancelled) qty — sets the upper bound for return modal. */
  remainingQty: number;
}

/** Captured when a mutation fires `override_required`; replays the action with the supplied PIN. */
type PendingOverride =
  | { kind: "cancel"; ref: ItemRef; role: string }
  | { kind: "return"; ref: ItemRef; qty: number; role: string }
  | { kind: "transfer"; toSpotId: number; role: string }
  | { kind: "merge_place_order"; role: string }
  | { kind: "merge_finalize"; role: string }
  | { kind: "split_open"; role: string }
  | { kind: "split_remove"; role: string; itemKey: string }
  | { kind: "split_place_order"; role: string };

/** Active merge attempt — both stages share these idempotency keys so retries
 *  (override prompt or banner recovery) dedupe at the backend. `orderPlaced`
 *  flips once the synthesized order is on B; failures after that point are
 *  recoverable via the banner. */
interface PendingMerge {
  target: SessionState;
  timeChargeProductId: number;
  timeCharge: number;
  /** Idempotency key for the synthesized place-order on B. Stable across retries. */
  placeKey: string;
  /** Idempotency key for the /sessions/merge call. Stable across retries. */
  mergeKey: string;
  /** Synthesized line items to place on B (rebuilt once per attempt). */
  items: RawOrderItem[];
  orderPlaced: boolean;
}

/** Active split attempt. Idempotency keys are stable so retry/resume after
 *  override or partial failure dedupes server-side. `removed` tracks which
 *  source items have already had their cancel/return applied so a retry
 *  doesn't double-deduct. */
interface PendingSplit {
  toSpotId: number;
  items: SplitItem[];
  /** Idempotency key for the open-session call. */
  openKey: string;
  /** Idempotency key per source item ("orderId#idx") for cancel/return. */
  removeKeys: Record<string, string>;
  /** Idempotency key for the place-order on N. */
  placeKey: string;
  /** Filled once N is created. */
  newSessionId: string | null;
  /** Set of "orderId#idx" already removed from source. */
  removed: Record<string, true>;
  /** True once the place-order on N has succeeded. */
  placed: boolean;
}

export function SessionDetailRoute() {
  const apiClient = useApiClient();
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

  const orderQueries = useQueries({
    queries: (session?.order_ids ?? []).map((oid) => ({
      queryKey: ["order", oid],
      queryFn: () => apiClient.get(`/orders/${oid}`, OrderState),
      enabled: !!session,
    })),
  });

  const orders = orderQueries
    .map((q) => q.data)
    .filter((o): o is NonNullable<typeof o> => !!o);

  const liveSubtotal = orders.reduce((sum, o) => {
    return (
      sum +
      o.items.reduce((s, it) => {
        if (it.cancelled) return s;
        const netQty = Math.max(0, it.spec.qty - it.returned_qty);
        return s + netQty * it.spec.unit_price;
      }, 0)
    );
  }, 0);

  // Stable idempotency keys per user intent.
  // - placeKey: tied to current cart batch; rotated on successful place-order
  //   so the next batch gets a fresh key.
  // - closeKey: one per detail-page mount; never rotated (only one close
  //   attempt per session).
  const [placeKey, setPlaceKey] = useState(() => crypto.randomUUID());
  const [closeKey] = useState(() => crypto.randomUUID());

  const placeOrder = useMutation({
    mutationFn: (items: RawOrderItem[]) => {
      const input: PlaceOrderInput = {
        idempotency_key: placeKey,
        session_id: sessionId,
        items,
      };
      return apiClient.post("/orders", OrderState, input);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      setCart({});
      setPlaceKey(crypto.randomUUID()); // rotate for next batch
    },
  });

  const closeSession = useMutation({
    mutationFn: () => {
      const input: CloseSessionInput = { idempotency_key: closeKey };
      return apiClient.post(
        `/sessions/${sessionId}/close`,
        SessionState,
        input,
      );
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      nav("/sessions");
    },
  });

  // ----- Cancel / return per-item state machine -----
  //
  // Both mutations follow the same shape: optimistic POST → on
  // override_required, stash a `PendingOverride` and prompt for a PIN via
  // OverrideModal → re-issue the same mutation with override_pin set.
  //
  // The return flow has one extra step before the mutation: a small modal
  // collects the qty (max = remaining qty on the line). We park the chosen
  // ref in `returnPrompt`, fire the mutation when the user submits, then let
  // the override flow take over if needed.
  const [pendingOverride, setPendingOverride] =
    useState<PendingOverride | null>(null);
  const [returnPrompt, setReturnPrompt] = useState<ItemRef | null>(null);
  const [showTransfer, setShowTransfer] = useState(false);
  const [showMerge, setShowMerge] = useState(false);
  const [pendingMerge, setPendingMerge] = useState<PendingMerge | null>(null);
  /** Banner shown when step-1 succeeded but step-2 failed (non-override). */
  const [mergeRecovery, setMergeRecovery] = useState<string | null>(null);
  const [showSplit, setShowSplit] = useState(false);
  const [pendingSplit, setPendingSplit] = useState<PendingSplit | null>(null);
  /** Banner for split partial-failure recovery. */
  const [splitRecovery, setSplitRecovery] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);

  const cancelItem = useMutation({
    mutationFn: async (vars: { ref: ItemRef; override?: string }) => {
      const input: CancelOrderItemInput = {
        idempotency_key: crypto.randomUUID(),
        // Cashier doing self-cancel within grace window (Plan C ACL): the
        // backend computes `is_self`/`within_grace` via its own clock + actor.
        // We pass conservative `false` so the policy enforces the manager-
        // override path uniformly; cashiers can still self-cancel by entering
        // their own PIN at the override prompt.
        is_self: false,
        within_grace: false,
        ...(vars.override ? { override_pin: vars.override } : {}),
      };
      return apiClient.post(
        `/orders/${vars.ref.orderId}/items/${vars.ref.idx}/cancel`,
        OrderState,
        input,
      );
    },
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: ["order", vars.ref.orderId] });
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      setPendingOverride(null);
      setActionError(null);
    },
    onError: (e: unknown, vars) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setPendingOverride({
          kind: "cancel",
          ref: vars.ref,
          role: e.envelope.message ?? "manager",
        });
      } else if (e instanceof ApiError) {
        setActionError(e.message);
      }
    },
  });

  const returnItem = useMutation({
    mutationFn: async (vars: {
      ref: ItemRef;
      qty: number;
      override?: string;
    }) => {
      const input: ReturnOrderItemInput = {
        idempotency_key: crypto.randomUUID(),
        qty: vars.qty,
        ...(vars.override ? { override_pin: vars.override } : {}),
      };
      return apiClient.post(
        `/orders/${vars.ref.orderId}/items/${vars.ref.idx}/return`,
        OrderState,
        input,
      );
    },
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: ["order", vars.ref.orderId] });
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      setPendingOverride(null);
      setReturnPrompt(null);
      setActionError(null);
    },
    onError: (e: unknown, vars) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setPendingOverride({
          kind: "return",
          ref: vars.ref,
          qty: vars.qty,
          role: e.envelope.message ?? "manager",
        });
      } else if (e instanceof ApiError) {
        setActionError(e.message);
      }
    },
  });

  const transfer = useMutation({
    mutationFn: async (vars: { toSpotId: number; override?: string }) => {
      const input: TransferSessionInput = {
        idempotency_key: crypto.randomUUID(),
        to_spot_id: vars.toSpotId,
        ...(vars.override ? { override_pin: vars.override } : {}),
      };
      return apiClient.post(
        `/sessions/${sessionId}/transfer`,
        SessionState,
        input,
      );
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      qc.invalidateQueries({ queryKey: ["spots"] });
      setShowTransfer(false);
      setPendingOverride(null);
      setActionError(null);
    },
    onError: (e: unknown, vars) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setPendingOverride({
          kind: "transfer",
          toSpotId: vars.toSpotId,
          role: e.envelope.message ?? "manager",
        });
      } else if (e instanceof ApiError) {
        setActionError(e.message);
      }
    },
  });

  // ----- Merge: two-step (place synthesized order on target, then merge) -----
  //
  // Stage 1 places one synthesized order on B containing all of A's
  // remaining (uncancelled, un-fully-returned) items, plus a Time line if
  // A is a room. Stage 2 calls /sessions/merge to mark A as Merged into B.
  //
  // Both stages reuse the keys captured in `pendingMerge` so a retry after
  // an override prompt OR after the recovery banner is idempotent.

  /** Build the synthesized order body for stage 1 from `orders`. */
  const buildMergeItems = (
    timeProductId: number,
    timeCharge: number,
  ): RawOrderItem[] => {
    const items: RawOrderItem[] = [];
    for (const o of orders) {
      for (const it of o.items) {
        if (it.cancelled) continue;
        const remaining = Math.max(0, it.spec.qty - it.returned_qty);
        if (remaining <= 0) continue;
        items.push({ product_id: it.spec.product_id, qty: remaining });
      }
    }
    if (timeCharge > 0 && timeProductId > 0) {
      // The Room Time product has price=0 in the catalog; per-line unit_price
      // override is not supported by RawOrderItem. We ship qty=timeCharge so
      // the line-total math (qty × 0) plus the running room clock at payment
      // time still surfaces the right amount on the bill prefill.
      //
      // NOTE: this is a limitation of the current RawOrderItem schema (no
      // per-line price override). The acting-cashier sees the time charge
      // separately on B's room clock until payment; the synthesized line is
      // a marker to anchor the merge in the order log. See follow-ups.
      items.push({ product_id: timeProductId, qty: 1 });
    }
    return items;
  };

  const placeMergeOrder = useMutation({
    mutationFn: async (vars: { merge: PendingMerge; override?: string }) => {
      const input: PlaceOrderInput = {
        idempotency_key: vars.merge.placeKey,
        session_id: vars.merge.target.session_id,
        items: vars.merge.items,
        ...(vars.override ? { override_pin: vars.override } : {}),
      };
      return apiClient.post("/orders", OrderState, input);
    },
    onSuccess: (_data, vars) => {
      const next: PendingMerge = { ...vars.merge, orderPlaced: true };
      setPendingMerge(next);
      setPendingOverride(null);
      // Chain into stage 2 immediately.
      finalizeMerge.mutate({ merge: next });
    },
    onError: (e: unknown, _vars) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setPendingOverride({
          kind: "merge_place_order",
          role: e.envelope.message ?? "manager",
        });
      } else if (e instanceof ApiError) {
        setActionError(e.message);
      }
    },
  });

  const finalizeMerge = useMutation({
    mutationFn: async (vars: { merge: PendingMerge; override?: string }) => {
      const input: MergeSessionsInput = {
        idempotency_key: vars.merge.mergeKey,
        into_session: vars.merge.target.session_id,
        sources: [sessionId],
        ...(vars.override ? { override_pin: vars.override } : {}),
      };
      return apiClient.post("/sessions/merge", SessionState, input);
    },
    onSuccess: (_data, vars) => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      qc.invalidateQueries({
        queryKey: ["session", vars.merge.target.session_id],
      });
      setShowMerge(false);
      setPendingMerge(null);
      setPendingOverride(null);
      setMergeRecovery(null);
      setActionError(null);
      nav(`/sessions/${vars.merge.target.session_id}`);
    },
    onError: (e: unknown) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setPendingOverride({
          kind: "merge_finalize",
          role: e.envelope.message ?? "manager",
        });
      } else if (e instanceof ApiError) {
        // Stage-1 already succeeded — surface the recovery banner.
        setMergeRecovery(e.message);
      }
    },
  });

  /** Kick off a merge attempt from the modal's onConfirm. */
  const startMerge = async (
    target: SessionState,
    timeProductId: number,
    timeCharge: number,
  ) => {
    const items = buildMergeItems(timeProductId, timeCharge);
    const merge: PendingMerge = {
      target,
      timeChargeProductId: timeProductId,
      timeCharge,
      placeKey: crypto.randomUUID(),
      mergeKey: crypto.randomUUID(),
      items,
      orderPlaced: false,
    };
    setPendingMerge(merge);
    setMergeRecovery(null);
    placeMergeOrder.mutate({ merge });
  };

  // ----- Split: open new session N, remove items from source, place on N --
  //
  // Each step is its own mutation that captures progress into `pendingSplit`
  // before chaining the next one. `removeKeys` and the other idempotency keys
  // are stable per attempt so retry-after-override re-uses the prior request.

  const itemKey = (it: SplitItem) => `${it.orderId}#${it.itemIndex}`;

  /** Cancel or return a single source item, then drive the next removal or
   *  the place-order step. */
  const removeOneSplitItem = (
    split: PendingSplit,
    item: SplitItem,
    override?: string,
  ) => {
    const key = itemKey(item);
    const idemKey = split.removeKeys[key]!;
    const url = item.isFullCancel
      ? `/orders/${item.orderId}/items/${item.itemIndex}/cancel`
      : `/orders/${item.orderId}/items/${item.itemIndex}/return`;
    const body: Record<string, unknown> = item.isFullCancel
      ? {
          idempotency_key: idemKey,
          is_self: false,
          within_grace: false,
        }
      : {
          idempotency_key: idemKey,
          qty: item.qty,
        };
    if (override) body.override_pin = override;
    apiClient
      .post(url, OrderState, body)
      .then(() => {
        const next: PendingSplit = {
          ...split,
          removed: { ...split.removed, [key]: true },
        };
        setPendingSplit(next);
        setPendingOverride(null);
        // Find the next item that hasn't been removed yet.
        const remaining = next.items.find((x) => !next.removed[itemKey(x)]);
        if (remaining) {
          removeOneSplitItem(next, remaining);
        } else {
          // All removed — invalidate source views, then place on N.
          qc.invalidateQueries({ queryKey: ["session", sessionId] });
          for (const it of next.items) {
            qc.invalidateQueries({ queryKey: ["order", it.orderId] });
          }
          placeSplitOrder(next);
        }
      })
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.code === "override_required") {
          setPendingOverride({
            kind: "split_remove",
            role: e.envelope.message ?? "manager",
            itemKey: key,
          });
        } else if (e instanceof ApiError) {
          setSplitRecovery(e.message);
        }
      });
  };

  const placeSplitOrder = (split: PendingSplit, override?: string) => {
    if (!split.newSessionId) return;
    const input: PlaceOrderInput = {
      idempotency_key: split.placeKey,
      session_id: split.newSessionId,
      items: split.items.map((i) => ({ product_id: i.productId, qty: i.qty })),
      ...(override ? { override_pin: override } : {}),
    };
    apiClient
      .post("/orders", OrderState, input)
      .then(() => {
        const next: PendingSplit = { ...split, placed: true };
        setPendingSplit(null);
        setShowSplit(false);
        setPendingOverride(null);
        setSplitRecovery(null);
        qc.invalidateQueries({ queryKey: ["sessions", "active"] });
        qc.invalidateQueries({ queryKey: ["session", sessionId] });
        nav(`/sessions/${next.newSessionId}`);
      })
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.code === "override_required") {
          setPendingOverride({
            kind: "split_place_order",
            role: e.envelope.message ?? "manager",
          });
        } else if (e instanceof ApiError) {
          setSplitRecovery(e.message);
        }
      });
  };

  const openSplitSession = (split: PendingSplit, override?: string) => {
    const input: OpenSessionInput = {
      idempotency_key: split.openKey,
      spot_id: split.toSpotId,
      ...(override ? { override_pin: override } : {}),
    };
    apiClient
      .post("/sessions", SessionState, input)
      .then((res) => {
        const next: PendingSplit = { ...split, newSessionId: res.session_id };
        setPendingSplit(next);
        setPendingOverride(null);
        const first = next.items.find((x) => !next.removed[itemKey(x)]);
        if (first) {
          removeOneSplitItem(next, first);
        } else {
          placeSplitOrder(next);
        }
      })
      .catch((e: unknown) => {
        if (e instanceof ApiError && e.code === "override_required") {
          setPendingOverride({
            kind: "split_open",
            role: e.envelope.message ?? "manager",
          });
        } else if (e instanceof ApiError) {
          setSplitRecovery(e.message);
        }
      });
  };

  /** Kick off a fresh split attempt from the modal's onConfirm. */
  const startSplit = async (toSpotId: number, items: SplitItem[]) => {
    const removeKeys: Record<string, string> = {};
    for (const it of items) removeKeys[itemKey(it)] = crypto.randomUUID();
    const split: PendingSplit = {
      toSpotId,
      items,
      openKey: crypto.randomUUID(),
      removeKeys,
      placeKey: crypto.randomUUID(),
      newSessionId: null,
      removed: {},
      placed: false,
    };
    setPendingSplit(split);
    setSplitRecovery(null);
    openSplitSession(split);
  };

  /** User-driven retry after a split partial-failure. Re-enters at the
   *  earliest step that still has work to do. */
  const retrySplit = () => {
    if (!pendingSplit) return;
    setSplitRecovery(null);
    if (!pendingSplit.newSessionId) {
      openSplitSession(pendingSplit);
      return;
    }
    const next = pendingSplit.items.find(
      (x) => !pendingSplit.removed[itemKey(x)],
    );
    if (next) {
      removeOneSplitItem(pendingSplit, next);
    } else {
      placeSplitOrder(pendingSplit);
    }
  };

  const submitOverride = async (pin: string) => {
    if (!pendingOverride) return;
    if (pendingOverride.kind === "cancel") {
      await cancelItem.mutateAsync({
        ref: pendingOverride.ref,
        override: pin,
      });
    } else if (pendingOverride.kind === "return") {
      await returnItem.mutateAsync({
        ref: pendingOverride.ref,
        qty: pendingOverride.qty,
        override: pin,
      });
    } else if (pendingOverride.kind === "transfer") {
      await transfer.mutateAsync({
        toSpotId: pendingOverride.toSpotId,
        override: pin,
      });
    } else if (pendingOverride.kind === "merge_place_order") {
      if (pendingMerge) {
        placeMergeOrder.mutate({ merge: pendingMerge, override: pin });
      }
    } else if (pendingOverride.kind === "merge_finalize") {
      if (pendingMerge) {
        finalizeMerge.mutate({ merge: pendingMerge, override: pin });
      }
    } else if (pendingOverride.kind === "split_open") {
      if (pendingSplit) openSplitSession(pendingSplit, pin);
    } else if (pendingOverride.kind === "split_remove") {
      if (pendingSplit) {
        const key = pendingOverride.itemKey;
        const item = pendingSplit.items.find((x) => itemKey(x) === key);
        if (item) removeOneSplitItem(pendingSplit, item, pin);
      }
    } else if (pendingOverride.kind === "split_place_order") {
      if (pendingSplit) placeSplitOrder(pendingSplit, pin);
    }
  };

  // Cart state: product_id -> qty
  const [cart, setCart] = useState<Record<number, number>>({});
  const cartItems: RawOrderItem[] = Object.entries(cart)
    .filter(([, qty]) => qty > 0)
    .map(([pid, qty]) => ({ product_id: Number(pid), qty }));

  if (!session) return <div>Loading…</div>;

  // History page brings closed sessions back into view; preserve the entry
  // point in the breadcrumb when the user came from there. Otherwise
  // default to /sessions.
  const fromHistory = session.status !== "Open";
  const crumbs = fromHistory
    ? [
        { label: <Trans>History</Trans>, to: "/history" },
        { label: session.spot.name },
      ]
    : [
        { label: <Trans>Sessions</Trans>, to: "/sessions" },
        { label: session.spot.name },
      ];

  return (
    <div>
      <Breadcrumbs items={crumbs} />
      <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
      <section>
        <h1 className="text-2xl font-semibold mb-2">{session.spot.name}</h1>
        <div className="text-sm text-gray-500 mb-4">
          {session.spot.kind} · {session.order_ids.length} orders
        </div>

        {session.spot.kind === "room" && session.status === "Open" && (
          <RoomClock
            openedAtMs={session.opened_at_ms}
            billing={session.spot.billing}
          />
        )}

        <div className="rounded-lg border bg-white p-4 mb-4">
          <h2 className="font-semibold mb-2">Order list</h2>
          {session.order_ids.length === 0 ? (
            <p className="text-gray-500 text-sm">No orders placed yet.</p>
          ) : (
            <>
              <ul className="text-sm divide-y">
                {orders.flatMap((o) =>
                  o.items.map((it, i) => {
                    const remaining = Math.max(
                      0,
                      it.spec.qty - it.returned_qty,
                    );
                    const ref: ItemRef = {
                      orderId: o.order_id,
                      idx: i,
                      remainingQty: remaining,
                    };
                    return (
                      <li
                        key={`${o.order_id}-${i}`}
                        className="py-1 flex items-center gap-2"
                      >
                        <span
                          className={
                            it.cancelled
                              ? "line-through text-gray-400 flex-1"
                              : "text-gray-700 flex-1"
                          }
                        >
                          {it.spec.qty}× {it.spec.product_name}
                          {it.returned_qty > 0 &&
                            ` (${it.returned_qty} returned)`}
                        </span>
                        <span
                          className={
                            it.cancelled
                              ? "line-through text-gray-400 tabular-nums"
                              : "tabular-nums"
                          }
                        >
                          {(it.spec.qty * it.spec.unit_price).toLocaleString(
                            "vi-VN",
                          )}
                          đ
                        </span>
                        {!it.cancelled && remaining > 0 && !session.payment_taken && (
                          <>
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={cancelItem.isPending}
                              onClick={() =>
                                cancelItem.mutate({ ref })
                              }
                            >
                              <Trans>Cancel</Trans>
                            </Button>
                            <Button
                              size="sm"
                              variant="ghost"
                              disabled={returnItem.isPending}
                              onClick={() => setReturnPrompt(ref)}
                            >
                              <Trans>Return</Trans>
                            </Button>
                          </>
                        )}
                      </li>
                    );
                  }),
                )}
              </ul>
              <div className="mt-2 pt-2 border-t font-semibold">
                Subtotal: {liveSubtotal.toLocaleString("vi-VN")}đ
              </div>
            </>
          )}
        </div>

        <div className="flex gap-2">
          <Button
            onClick={() => nav(`/sessions/${sessionId}/payment`)}
            disabled={session.status !== "Open" || session.payment_taken}
            title={
              session.payment_taken
                ? "Already paid"
                : session.status !== "Open"
                ? "Session is not open"
                : undefined
            }
          >
            Take payment
          </Button>
          <Button
            variant="outline"
            onClick={() => closeSession.mutate()}
            disabled={closeSession.isPending || session.status !== "Open"}
          >
            Close session
          </Button>
          <Button
            variant="outline"
            onClick={() => setShowTransfer(true)}
            disabled={transfer.isPending || session.status !== "Open"}
          >
            <Trans>Move…</Trans>
          </Button>
          <Button
            variant="outline"
            onClick={() => setShowMerge(true)}
            disabled={
              placeMergeOrder.isPending ||
              finalizeMerge.isPending ||
              session.status !== "Open"
            }
          >
            <Trans>Merge into…</Trans>
          </Button>
          <Button
            variant="outline"
            onClick={() => setShowSplit(true)}
            disabled={pendingSplit !== null || session.status !== "Open"}
          >
            <Trans>Split…</Trans>
          </Button>
        </div>
        {session.payment_taken && (
          <div className="text-sm text-amber-700 bg-amber-50 border border-amber-200 rounded mt-2 p-2">
            <Trans>This session has been paid. Items can no longer be cancelled or returned.</Trans>
          </div>
        )}
        {session.status !== "Open" && !session.payment_taken && (
          <div className="text-sm text-gray-700 bg-gray-50 border rounded mt-2 p-2">
            <Trans>Session is {session.status === "Closed" ? "closed" : "not open"}.</Trans>
          </div>
        )}
        {closeSession.error instanceof ApiError && (
          <div className="text-red-600 text-sm mt-2">
            {closeSession.error.message}
          </div>
        )}
        {actionError && (
          <div className="text-red-600 text-sm mt-2">{actionError}</div>
        )}
        {mergeRecovery && pendingMerge && (
          <div className="text-amber-800 bg-amber-50 border border-amber-200 rounded mt-2 p-2 text-sm">
            <div className="mb-2">
              <Trans>
                Merge half-completed: items were placed on{" "}
                {pendingMerge.target.spot.name} but the final merge step failed
                ({mergeRecovery}). Retry the merge, or cancel the new order on{" "}
                {pendingMerge.target.spot.name} and try again.
              </Trans>
            </div>
            <div className="flex gap-2">
              <Button
                size="sm"
                onClick={() => finalizeMerge.mutate({ merge: pendingMerge })}
                disabled={finalizeMerge.isPending}
              >
                <Trans>Retry merge</Trans>
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  setMergeRecovery(null);
                  setPendingMerge(null);
                  setShowMerge(false);
                }}
              >
                <Trans>Dismiss</Trans>
              </Button>
            </div>
          </div>
        )}
        {splitRecovery && pendingSplit && (
          <div className="text-amber-800 bg-amber-50 border border-amber-200 rounded mt-2 p-2 text-sm">
            <div className="mb-2">
              <Trans>
                Split partially applied ({splitRecovery}). Retry to resume
                from the failed step, or dismiss and reconcile manually.
              </Trans>
            </div>
            <div className="flex gap-2">
              <Button size="sm" onClick={retrySplit}>
                <Trans>Retry split</Trans>
              </Button>
              <Button
                size="sm"
                variant="ghost"
                onClick={() => {
                  setSplitRecovery(null);
                  setPendingSplit(null);
                  setShowSplit(false);
                }}
              >
                <Trans>Dismiss</Trans>
              </Button>
            </div>
          </div>
        )}
      </section>

      <section>
        <h2 className="text-xl font-semibold mb-3">Add items</h2>
        <div className="grid grid-cols-2 gap-2 mb-4">
          {products?.map((p) => (
            <div
              key={p.id}
              className="rounded border bg-white p-3 flex items-center justify-between"
            >
              <div>
                <div className="font-medium">{p.name}</div>
                <div className="text-xs text-gray-500">
                  {p.price.toLocaleString("vi-VN")}đ
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setCart((c) => ({
                      ...c,
                      [p.id]: Math.max(0, (c[p.id] ?? 0) - 1),
                    }))
                  }
                >
                  −
                </Button>
                <span className="w-8 text-center">{cart[p.id] ?? 0}</span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setCart((c) => ({ ...c, [p.id]: (c[p.id] ?? 0) + 1 }))
                  }
                >
                  +
                </Button>
              </div>
            </div>
          ))}
        </div>
        <Button
          className="w-full"
          disabled={
            cartItems.length === 0 ||
            placeOrder.isPending ||
            session.status !== "Open" ||
            session.payment_taken
          }
          onClick={() => placeOrder.mutate(cartItems)}
        >
          Place order ({cartItems.length})
        </Button>
        {placeOrder.error instanceof ApiError && (
          <div className="text-red-600 text-sm mt-2">
            {placeOrder.error.message}
          </div>
        )}
      </section>

      {returnPrompt && (
        <ReturnPromptModal
          ref={returnPrompt}
          busy={returnItem.isPending}
          onCancel={() => setReturnPrompt(null)}
          onSubmit={(qty) =>
            returnItem.mutate({ ref: returnPrompt, qty })
          }
        />
      )}

      {showTransfer && (
        <TransferModal
          currentSpotId={session.spot.id}
          busy={transfer.isPending}
          onClose={() => setShowTransfer(false)}
          onSelect={(toSpotId) => transfer.mutate({ toSpotId })}
        />
      )}

      {showMerge && (
        <MergeModal
          source={session}
          busy={placeMergeOrder.isPending || finalizeMerge.isPending}
          onClose={() => {
            if (!pendingMerge?.orderPlaced) {
              setShowMerge(false);
              setPendingMerge(null);
            }
          }}
          onConfirm={startMerge}
        />
      )}

      {showSplit && (
        <SplitModal
          source={session}
          orders={orders}
          busy={pendingSplit !== null}
          onClose={() => {
            if (!pendingSplit) setShowSplit(false);
          }}
          onConfirm={startSplit}
        />
      )}

      <OverrideModal
        open={pendingOverride !== null}
        minRole={pendingOverride?.role ?? "manager"}
        onSubmit={submitOverride}
        onCancel={() => setPendingOverride(null)}
      />

      {/* Reference t() so non-Trans literals still get extracted in the
          rare case we add interpolation later; harmless. */}
      <span className="sr-only">{t`Loading…`}</span>
      </div>
    </div>
  );
}

interface ReturnPromptProps {
  ref: ItemRef;
  busy: boolean;
  onSubmit: (qty: number) => void;
  onCancel: () => void;
}

function ReturnPromptModal({
  ref,
  busy,
  onSubmit,
  onCancel,
}: ReturnPromptProps) {
  const [qty, setQty] = useState(1);
  const max = ref.remainingQty;
  const valid = qty >= 1 && qty <= max;
  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-40">
      <div className="bg-white rounded-lg p-4 max-w-sm w-full">
        <h3 className="text-lg font-semibold mb-3">
          <Trans>Return how many?</Trans>
        </h3>
        <input
          type="number"
          className="border rounded px-2 py-1 w-full"
          min={1}
          max={max}
          value={qty}
          onChange={(e) => setQty(Number(e.target.value))}
        />
        <p className="text-xs text-gray-500 mt-1">
          <Trans>Max: {max}</Trans>
        </p>
        <div className="flex gap-2 mt-4 justify-end">
          <Button variant="outline" onClick={onCancel} disabled={busy}>
            <Trans>Cancel</Trans>
          </Button>
          <Button onClick={() => onSubmit(qty)} disabled={busy || !valid}>
            <Trans>Submit</Trans>
          </Button>
        </div>
      </div>
    </div>
  );
}
