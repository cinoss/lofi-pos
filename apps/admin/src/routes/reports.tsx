import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { useApiClient } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { DailyReport, DailyReportList } from "@lofi-pos/shared";
import { Modal } from "../lib/modal";

export function ReportsRoute() {
  const api = useApiClient();
  const [openDay, setOpenDay] = useState<string | null>(null);

  const list = useQuery({
    queryKey: ["admin", "reports"],
    queryFn: () => api.get("/admin/reports", DailyReportList),
  });

  const detail = useQuery({
    queryKey: ["admin", "reports", openDay],
    queryFn: () => api.get(`/admin/reports/${openDay}`, DailyReport),
    enabled: openDay != null,
  });

  return (
    <div>
      <h1 className="mb-4 text-2xl font-semibold">Daily reports</h1>
      {list.isLoading && <p>Loading…</p>}
      {list.error && <p className="text-red-600">{String(list.error)}</p>}
      <table className="w-full border-collapse rounded-lg bg-white shadow-sm">
        <thead className="border-b bg-gray-100 text-left text-sm">
          <tr>
            <th className="p-3">Business day</th>
            <th className="p-3">Generated at</th>
            <th className="p-3" />
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((r) => (
            <tr key={r.business_day} className="border-b text-sm last:border-b-0">
              <td className="p-3 font-mono">{r.business_day}</td>
              <td className="p-3">{new Date(r.generated_at).toLocaleString()}</td>
              <td className="p-3 text-right">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() => setOpenDay(r.business_day)}
                >
                  View JSON
                </Button>
              </td>
            </tr>
          ))}
          {list.data?.length === 0 && (
            <tr>
              <td className="p-3 text-sm text-gray-500" colSpan={3}>
                No reports yet — they appear after the next end-of-day run.
              </td>
            </tr>
          )}
        </tbody>
      </table>

      <Modal
        open={openDay != null}
        title={`Report for ${openDay ?? ""}`}
        onClose={() => setOpenDay(null)}
      >
        {detail.isLoading && <p>Loading…</p>}
        {detail.data && (
          <pre className="max-h-[60vh] overflow-auto rounded bg-gray-100 p-3 text-xs">
            {prettyJson(detail.data.order_summary_json)}
          </pre>
        )}
      </Modal>
    </div>
  );
}

function prettyJson(raw: string): string {
  try {
    return JSON.stringify(JSON.parse(raw), null, 2);
  } catch {
    return raw;
  }
}
