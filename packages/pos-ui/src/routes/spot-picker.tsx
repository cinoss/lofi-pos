import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useNavigate } from "react-router-dom";
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

  const open = useMutation({
    mutationFn: (input: OpenSessionInput) =>
      apiClient.post("/sessions", SessionState, input),
    onSuccess: (session) => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      nav(`/sessions/${session.session_id}`);
    },
  });

  if (!spots) return <div>Loading…</div>;

  const idle = spots.filter((s) => s.status === "idle");

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-4">Open new session</h1>
      <div className="grid grid-cols-2 sm:grid-cols-3 lg:grid-cols-4 gap-3">
        {idle.map((s) => (
          <Button
            key={s.id}
            variant="outline"
            className="h-24 flex flex-col"
            disabled={open.isPending}
            onClick={() =>
              open.mutate({
                idempotency_key: newKey(),
                spot_id: s.id,
              })
            }
          >
            <span className="text-lg font-semibold">{s.name}</span>
            <span className="text-xs text-gray-500">{s.kind}</span>
          </Button>
        ))}
      </div>
      {idle.length === 0 && <p className="text-gray-500">No idle spots.</p>}
    </div>
  );
}
