import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { z } from "zod";
import { Trans } from "@lingui/react/macro";
import { Spot, SessionState, OrderState } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { useApiClient } from "../api-context";

export interface SplitItem {
  orderId: string;
  itemIndex: number;
  /** Product id snapshot — we ship this on the new order so the new session
   *  has the same items at the catalog's current price. */
  productId: number;
  /** Display only — `it.spec.product_name`. */
  productName: string;
  /** How many units of this line to move. Bounded by remaining qty. */
  qty: number;
  /** True iff `qty == remaining` — backend takes a different path
   *  (cancel vs return) so the caller can dispatch correctly. */
  isFullCancel: boolean;
}

interface SplitModalProps {
  source: SessionState;
  /** Source's orders, already loaded by caller. */
  orders: OrderState[];
  onClose: () => void;
  onConfirm: (toSpotId: number, items: SplitItem[]) => Promise<void> | void;
  busy?: boolean;
}

interface PickerRow {
  orderId: string;
  itemIndex: number;
  productId: number;
  productName: string;
  remaining: number;
}

/**
 * Two-section modal: spot picker (idle only, current spot excluded) plus
 * a per-line qty picker for source's open items. Caller takes
 * (chosenSpotId, items) and runs the open-session + cancel/return + place-order
 * sequence; this modal only collects intent.
 */
export function SplitModal({
  source,
  orders,
  onClose,
  onConfirm,
  busy = false,
}: SplitModalProps) {
  const apiClient = useApiClient();
  const { data: spots } = useQuery({
    queryKey: ["spots"],
    queryFn: () => apiClient.get("/spots", z.array(Spot)),
  });
  const { data: actives } = useQuery({
    queryKey: ["sessions", "active"],
    queryFn: () => apiClient.get("/sessions/active", z.array(SessionState)),
  });

  const occupied = new Set<number>();
  for (const s of actives ?? []) {
    if (s.status === "Open") occupied.add(s.spot.id);
  }
  const candidates = (spots ?? []).filter(
    (s) => s.id !== source.spot.id && !occupied.has(s.id),
  );

  const rows: PickerRow[] = [];
  for (const o of orders) {
    o.items.forEach((it, idx) => {
      if (it.cancelled) return;
      const remaining = Math.max(0, it.spec.qty - it.returned_qty);
      if (remaining <= 0) return;
      rows.push({
        orderId: o.order_id,
        itemIndex: idx,
        productId: it.spec.product_id,
        productName: it.spec.product_name,
        remaining,
      });
    });
  }

  // Map "orderId#idx" → chosen qty (0 = unselected).
  const [picks, setPicks] = useState<Record<string, number>>({});
  const [toSpotId, setToSpotId] = useState<number | null>(null);

  const items: SplitItem[] = rows
    .map((r) => {
      const key = `${r.orderId}#${r.itemIndex}`;
      const qty = picks[key] ?? 0;
      if (qty <= 0) return null;
      return {
        orderId: r.orderId,
        itemIndex: r.itemIndex,
        productId: r.productId,
        productName: r.productName,
        qty,
        isFullCancel: qty >= r.remaining,
      } satisfies SplitItem;
    })
    .filter((x): x is SplitItem => x !== null);

  const canConfirm = toSpotId !== null && items.length > 0;

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-6 max-w-2xl w-full max-h-[90vh] overflow-auto">
        <h2 className="text-xl font-semibold mb-3">
          <Trans>Split…</Trans>
        </h2>

        <div className="mb-4">
          <h3 className="font-semibold mb-2">
            <Trans>Move to spot</Trans>
          </h3>
          {candidates.length === 0 ? (
            <p className="text-gray-500 text-sm">
              <Trans>No idle spots available.</Trans>
            </p>
          ) : (
            <ul className="grid grid-cols-3 gap-2">
              {candidates.map((s) => (
                <li key={s.id}>
                  <Button
                    variant={toSpotId === s.id ? "default" : "outline"}
                    className="w-full h-14 flex flex-col"
                    disabled={busy}
                    onClick={() => setToSpotId(s.id)}
                  >
                    <span className="font-semibold">{s.name}</span>
                    <span className="text-xs text-gray-500">{s.kind}</span>
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <div className="mb-4">
          <h3 className="font-semibold mb-2">
            <Trans>Move which items?</Trans>
          </h3>
          {rows.length === 0 ? (
            <p className="text-gray-500 text-sm">
              <Trans>No items to move.</Trans>
            </p>
          ) : (
            <ul className="text-sm divide-y border rounded">
              {rows.map((r) => {
                const key = `${r.orderId}#${r.itemIndex}`;
                const qty = picks[key] ?? 0;
                const checked = qty > 0;
                return (
                  <li key={key} className="flex items-center gap-2 px-2 py-1">
                    <input
                      type="checkbox"
                      checked={checked}
                      disabled={busy}
                      onChange={(e) =>
                        setPicks((p) => ({
                          ...p,
                          [key]: e.target.checked ? r.remaining : 0,
                        }))
                      }
                    />
                    <span className="flex-1">
                      {r.productName}{" "}
                      <span className="text-gray-500">(×{r.remaining})</span>
                    </span>
                    <input
                      type="number"
                      className="border rounded px-2 py-1 w-20"
                      min={0}
                      max={r.remaining}
                      value={qty}
                      disabled={busy}
                      onChange={(e) => {
                        const n = Math.max(
                          0,
                          Math.min(r.remaining, Number(e.target.value) || 0),
                        );
                        setPicks((p) => ({ ...p, [key]: n }));
                      }}
                    />
                  </li>
                );
              })}
            </ul>
          )}
        </div>

        <div className="flex gap-2 justify-end">
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            <Trans>Cancel</Trans>
          </Button>
          <Button
            onClick={() => {
              if (!canConfirm) return;
              void onConfirm(toSpotId!, items);
            }}
            disabled={busy || !canConfirm}
          >
            <Trans>Confirm split</Trans>
          </Button>
        </div>
      </div>
    </div>
  );
}
