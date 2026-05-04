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
  /**
   * Caller drives the two-step merge (place-order on target, then
   * /sessions/merge). `timeChargeProductId` is 0 when no time charge applies.
   */
  onConfirm: (
    target: SessionState,
    timeChargeProductId: number,
    timeCharge: number,
  ) => Promise<void> | void;
  busy?: boolean;
}

/**
 * Two-step picker: first pick a target Open session, then preview the
 * room-time line that will be added to it. Cross-references active sessions
 * (status=Open) to populate the target list. Looks up the seeded
 * `kind=time` product to use as the line item; if missing we surface an
 * inline error and disable Confirm so the user runs setup or adds one.
 */
export function MergeModal({
  source,
  onClose,
  onConfirm,
  busy = false,
}: MergeModalProps) {
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
        <h2 className="text-xl font-semibold mb-3">
          <Trans>Merge into…</Trans>
        </h2>
        {!picked ? (
          <>
            {targets.length === 0 ? (
              <p className="text-gray-500 text-sm">
                <Trans>No other open sessions.</Trans>
              </p>
            ) : (
              <ul className="grid grid-cols-2 gap-2 max-h-80 overflow-auto">
                {targets.map((t) => (
                  <li key={t.session_id}>
                    <Button
                      variant="outline"
                      className="w-full h-16 flex flex-col"
                      disabled={busy}
                      onClick={() => setPicked(t)}
                    >
                      <span className="font-semibold">{t.spot.name}</span>
                      <span className="text-xs text-gray-500">
                        {t.spot.kind}
                      </span>
                    </Button>
                  </li>
                ))}
              </ul>
            )}
            <div className="mt-4 text-right">
              <Button variant="ghost" onClick={onClose} disabled={busy}>
                <Trans>Cancel</Trans>
              </Button>
            </div>
          </>
        ) : (
          <>
            <div className="mb-3 text-sm text-gray-700">
              <Trans>
                Move {source.order_ids.length} order(s) and{" "}
                {sourceTimeCharge.toLocaleString("vi-VN")}đ room time from{" "}
                <b>{source.spot.name}</b> into <b>{picked.spot.name}</b>.
              </Trans>
            </div>
            {sourceTimeCharge > 0 && !timeProduct && (
              <div className="text-red-600 text-sm mb-3">
                <Trans>
                  No "Room Time" product configured. Run setup or add a product
                  with kind=time.
                </Trans>
              </div>
            )}
            <div className="flex gap-2 justify-end">
              <Button
                variant="outline"
                onClick={() => setPicked(null)}
                disabled={busy}
              >
                <Trans>Back</Trans>
              </Button>
              <Button
                onClick={() => {
                  if (sourceTimeCharge > 0 && !timeProduct) return;
                  void onConfirm(
                    picked,
                    timeProduct?.id ?? 0,
                    sourceTimeCharge,
                  );
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
