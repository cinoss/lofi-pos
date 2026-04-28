import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { z } from "zod";
import { SessionState } from "@lofi-pos/shared";
import { useApiClient } from "../api-context";

export function SessionsRoute() {
  const apiClient = useApiClient();
  const { data, isLoading, error } = useQuery({
    queryKey: ["sessions", "active"],
    queryFn: () => apiClient.get("/sessions/active", z.array(SessionState)),
  });

  if (isLoading) return <div>Loading…</div>;
  if (error) return <div className="text-red-600">Error: {String(error)}</div>;

  if (!data || data.length === 0) {
    return (
      <div className="text-center">
        <p className="text-gray-500">No active sessions.</p>
        <Link
          to="/spots"
          className="mt-4 inline-block text-blue-600 hover:underline"
        >
          Open new session →
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
            {s.spot.kind === "room" ? "Room" : "Table"} ·{" "}
            {s.customer_label ?? "—"}
          </div>
          <div className="mt-2 text-xs text-gray-400">
            {s.order_ids.length} orders
          </div>
        </Link>
      ))}
    </div>
  );
}
