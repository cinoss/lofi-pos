import { useQuery } from "@tanstack/react-query";
import { z } from "zod";
import { Trans } from "@lingui/react/macro";
import { Spot } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { useApiClient } from "../api-context";

interface TransferModalProps {
  /** Spot the session is currently parked at; excluded from the picker. */
  currentSpotId: number;
  onClose: () => void;
  onSelect: (spotId: number) => void;
  /** Disable the picker tiles while a transfer mutation is in flight. */
  busy?: boolean;
}

/**
 * Spot picker for `POST /sessions/:id/transfer`. Filters server-side `/spots`
 * to idle, non-current candidates so the staffer only sees somewhere a
 * session could legally move to. Transfer itself (with override flow) is
 * driven from session-detail; this modal is just the chooser.
 */
export function TransferModal({
  currentSpotId,
  onClose,
  onSelect,
  busy = false,
}: TransferModalProps) {
  const apiClient = useApiClient();
  const { data: spots } = useQuery({
    queryKey: ["spots"],
    queryFn: () => apiClient.get("/spots", z.array(Spot)),
  });

  const candidates = (spots ?? []).filter(
    (s) => s.id !== currentSpotId && s.status === "idle",
  );

  return (
    <div className="fixed inset-0 bg-black/40 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-6 max-w-md w-full">
        <h2 className="text-xl font-semibold mb-3">
          <Trans>Move to spot</Trans>
        </h2>
        {candidates.length === 0 ? (
          <p className="text-gray-500 text-sm">
            <Trans>No idle spots available.</Trans>
          </p>
        ) : (
          <ul className="grid grid-cols-2 gap-2 max-h-80 overflow-auto">
            {candidates.map((s) => (
              <li key={s.id}>
                <Button
                  variant="outline"
                  className="w-full h-16 flex flex-col"
                  disabled={busy}
                  onClick={() => onSelect(s.id)}
                >
                  <span className="font-semibold">{s.name}</span>
                  <span className="text-xs text-gray-500">{s.kind}</span>
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
      </div>
    </div>
  );
}
