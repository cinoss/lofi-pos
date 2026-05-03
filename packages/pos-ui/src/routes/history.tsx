import { useQuery } from "@tanstack/react-query";
import { Link } from "react-router-dom";
import { Trans, Plural } from "@lingui/react/macro";
import { z } from "zod";
import { SessionState } from "@lofi-pos/shared";
import { useApiClient } from "../api-context";

/**
 * Sessions in any non-Open status (Closed, Merged, Split). Bounded by the
 * in-memory aggregate store — sessions whose events have been deleted at
 * EOD won't appear here. For deeper history (past EOD) the venue must
 * pull from the bouncer's report storage.
 */
export function HistoryRoute() {
  const apiClient = useApiClient();
  const { data, isLoading, error } = useQuery({
    queryKey: ["sessions", "history"],
    queryFn: () => apiClient.get("/sessions/history", z.array(SessionState)),
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
      <div className="text-center mt-8">
        <p className="text-gray-500">
          <Trans>No history yet — closed sessions will show up here.</Trans>
        </p>
      </div>
    );
  }

  return (
    <div>
      <h1 className="text-2xl font-semibold mb-4">
        <Trans>History</Trans>
      </h1>
      <p className="text-xs text-gray-500 mb-4">
        <Trans>
          Closed sessions still in the local event store. Older history is
          available in the daily reports stored by the bouncer service.
        </Trans>
      </p>
      <div className="grid grid-cols-1 sm:grid-cols-2 lg:grid-cols-3 gap-4">
        {data.map((s) => {
          const badge = s.payment_taken
            ? { text: <Trans>Paid</Trans>, cls: "bg-amber-100 text-amber-800" }
            : { text: <Trans>{String(s.status)}</Trans>, cls: "bg-gray-200 text-gray-700" };
          const opened = new Date(s.opened_at_ms).toLocaleString("vi-VN");
          return (
            <Link
              key={s.session_id}
              to={`/sessions/${s.session_id}`}
              className="rounded-lg border bg-white p-4 hover:border-blue-400 hover:shadow"
            >
              <div className="flex items-center justify-between">
                <div className="font-semibold">{s.spot.name}</div>
                <span className={`text-xs px-2 py-0.5 rounded ${badge.cls}`}>
                  {badge.text}
                </span>
              </div>
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
              <div className="mt-1 text-xs text-gray-400">{opened}</div>
            </Link>
          );
        })}
      </div>
    </div>
  );
}
