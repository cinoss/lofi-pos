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
  | { kind: "transfer"; toSpotId: number; role: string };

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
    } else {
      await transfer.mutateAsync({
        toSpotId: pendingOverride.toSpotId,
        override: pin,
      });
    }
  };

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

        {session.spot.kind === "room" && session.status === "Open" && (
          <RoomClock
            openedAtMs={session.opened_at_ms}
            hourlyRate={session.spot.hourly_rate}
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
