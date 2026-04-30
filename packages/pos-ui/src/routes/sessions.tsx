import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { Trans, Plural } from "@lingui/react/macro";
import { z } from "zod";
import { SessionState } from "@lofi-pos/shared";
import { useApiClient } from "../api-context";

export function SessionsRoute() {
  const apiClient = useApiClient();
  const { data, isLoading, error } = useQuery({
    queryKey: ["sessions", "active"],
    queryFn: () => apiClient.get("/sessions/active", z.array(SessionState)),
  });

  if (isLoading) return <div><Trans>Loading…</Trans></div>;
  if (error)
    return (
      <div className="text-red-600">
        <Trans>Error: {String(error)}</Trans>
      </div>
    );

  if (!data || data.length === 0) {
    return (
      <div className="text-center">
        <p className="text-gray-500">
          <Trans>No active sessions.</Trans>
        </p>
        <Link
          to="/spots"
          className="mt-4 inline-block text-blue-600 hover:underline"
        >
          <Trans>Open new session →</Trans>
        </Link>
      </div>
    );
  }

  return (
    <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
      {data.map((s) => (
        <Link
          key={s.session_id}
          to={`/sessions/${s.session_id}`}
          className="rounded-lg border bg-white p-4 hover:border-blue-400 hover:shadow"
        >
          <div className="font-semibold">{s.spot.name}</div>
          <div className="text-sm text-gray-500">
            {s.spot.kind === "room" ? <Trans>Room</Trans> : <Trans>Table</Trans>}{" "}
            · {s.customer_label ?? "—"}
          </div>
          <div className="mt-2 text-xs text-gray-400">
            <Plural
              value={s.order_ids.length}
              one="# order"
              other="# orders"
            />
          </div>
        </Link>
      ))}
    </div>
  );
}
