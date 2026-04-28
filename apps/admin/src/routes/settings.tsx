import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiClient } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { Settings, SettingsUpdate } from "@lofi-pos/shared";

interface FormState {
  business_day_cutoff_hour: string;
  business_day_tz_offset_seconds: string;
  discount_threshold_pct: string;
  cancel_grace_minutes: string;
  idle_lock_minutes: string;
}

export function SettingsRoute() {
  const api = useApiClient();
  const qc = useQueryClient();
  const [form, setForm] = useState<FormState | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saved, setSaved] = useState(false);

  const q = useQuery({
    queryKey: ["admin", "settings"],
    queryFn: () => api.get("/admin/settings", Settings),
  });

  // Mirror loaded settings into the form once.
  useEffect(() => {
    if (q.data && form == null) {
      setForm({
        business_day_cutoff_hour: String(q.data.business_day_cutoff_hour),
        business_day_tz_offset_seconds: String(q.data.business_day_tz_offset_seconds),
        discount_threshold_pct: String(q.data.discount_threshold_pct),
        cancel_grace_minutes: String(q.data.cancel_grace_minutes),
        idle_lock_minutes: String(q.data.idle_lock_minutes),
      });
    }
  }, [q.data, form]);

  const save = useMutation({
    mutationFn: async (f: FormState) => {
      const payload = SettingsUpdate.parse({
        business_day_cutoff_hour: Number(f.business_day_cutoff_hour),
        business_day_tz_offset_seconds: Number(f.business_day_tz_offset_seconds),
        discount_threshold_pct: Number(f.discount_threshold_pct),
        cancel_grace_minutes: Number(f.cancel_grace_minutes),
        idle_lock_minutes: Number(f.idle_lock_minutes),
      });
      await api.put("/admin/settings", Settings, payload);
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["admin", "settings"] });
      setError(null);
      setSaved(true);
    },
    onError: (e: unknown) => setError(e instanceof Error ? e.message : "save failed"),
  });

  if (q.isLoading || form == null) return <p>Loading…</p>;

  return (
    <div className="max-w-lg">
      <h1 className="mb-4 text-2xl font-semibold">Settings</h1>
      <form
        className="space-y-3 rounded-lg bg-white p-6 shadow-sm"
        onSubmit={(e) => {
          e.preventDefault();
          setSaved(false);
          save.mutate(form);
        }}
      >
        {(
          [
            ["business_day_cutoff_hour", "Business-day cutoff hour (0–23)"],
            ["business_day_tz_offset_seconds", "TZ offset (seconds, e.g. 25200 = +07:00)"],
            ["discount_threshold_pct", "Discount threshold (%)"],
            ["cancel_grace_minutes", "Cancel grace (minutes)"],
            ["idle_lock_minutes", "Idle lock (minutes)"],
          ] as const
        ).map(([key, label]) => (
          <label key={key} className="block text-sm">
            {label}
            <input
              className="mt-1 block w-full rounded border px-2 py-1"
              value={form[key]}
              onChange={(e) => setForm({ ...form, [key]: e.target.value })}
              inputMode="numeric"
              required
            />
          </label>
        ))}
        {error && <p className="text-sm text-red-600">{error}</p>}
        {saved && !error && <p className="text-sm text-green-600">Saved.</p>}
        <div className="flex justify-end">
          <Button type="submit" disabled={save.isPending}>
            Save
          </Button>
        </div>
      </form>
    </div>
  );
}
