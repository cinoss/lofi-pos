import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
import { Trans } from "@lingui/react/macro";
import { z } from "zod";
import { Spot, SessionState } from "@lofi-pos/shared";
import type { OpenSessionInput } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { useApiClient } from "../api-context";

function newKey(): string {
  return crypto.randomUUID();
}

export function SpotPickerRoute() {
  const apiClient = useApiClient();
  const qc = useQueryClient();
  const nav = useNavigate();
  const { data: spots } = useQuery({
    queryKey: ["spots"],
    queryFn: () => apiClient.get("/spots", z.array(Spot)),
  });

  // Open sessions tell us which spots are actually occupied (master.spot.status
  // is not maintained by the session lifecycle today; cross-reference instead).
  const { data: activeSessions } = useQuery({
    queryKey: ["sessions", "active"],
    queryFn: () => apiClient.get("/sessions/active", z.array(SessionState)),
  });

  const open = useMutation({
    mutationFn: (input: OpenSessionInput) =>
      apiClient.post("/sessions", SessionState, input),
    onSuccess: (session) => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      nav(`/sessions/${session.session_id}`);
    },
  });

  if (!spots || !activeSessions)
    return <div><Trans>Loading…</Trans></div>;

  const occupiedBy = new Map<number, string>();
  for (const s of activeSessions) {
    if (s.status === "Open") occupiedBy.set(s.spot.id, s.session_id);
  }
  const free = spots.filter((s) => !occupiedBy.has(s.id));

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-4">
        <Trans>Open new session</Trans>
      </h1>
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
        {spots.map((s) => {
          const sessionId = occupiedBy.get(s.id);
          const isOccupied = !!sessionId;
          return (
            <Button
              key={s.id}
              variant={isOccupied ? "secondary" : "outline"}
              className="h-24 flex flex-col"
              disabled={open.isPending || isOccupied}
              onClick={() =>
                open.mutate({
                  idempotency_key: newKey(),
                  spot_id: s.id,
                })
              }
              title={isOccupied ? "Occupied" : undefined}
            >
              <span className="text-lg font-semibold">{s.name}</span>
              <span className="text-xs text-gray-500">
                {s.kind}
                {isOccupied && (
                  <>
                    {" · "}
                    <Trans>occupied</Trans>
                  </>
                )}
              </span>
            </Button>
          );
        })}
      </div>
      {spots.length === 0 ? (
        <p className="text-gray-500 mt-4">
          <Trans>
            No spots configured yet. Open the admin app and add at least one
            room or table.
          </Trans>
        </p>
      ) : free.length === 0 ? (
        <p className="text-gray-500 mt-4">
          <Trans>No idle spots — all spots have an open session.</Trans>
        </p>
      ) : null}
    </div>
  );
}
