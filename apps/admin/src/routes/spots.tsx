import { useState } from "react";
import { z } from "zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";
import { useApiClient } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { Spot, SpotInput } from "@lofi-pos/shared";
import { Modal } from "../lib/modal";

const SpotList = z.array(Spot);

interface FormState {
  id: number | null;
  name: string;
  kind: "room" | "table";
  // Billing fields are stored as strings while editing; only forwarded
  // when kind === "room". For tables they're ignored and the request
  // sends `billing_config: null`.
  hourly_rate: string;
  bucket_minutes: string;
  included_minutes: string;
  min_charge: string;
  parent_id: string;
}

const empty: FormState = {
  id: null,
  name: "",
  kind: "table",
  hourly_rate: "",
  bucket_minutes: "1",
  included_minutes: "0",
  min_charge: "0",
  parent_id: "",
};

export function SpotsRoute() {
  const api = useApiClient();
  const qc = useQueryClient();
  const [form, setForm] = useState<FormState | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: ["admin", "spots"],
    queryFn: () => api.get("/admin/spots", SpotList),
  });

  const upsert = useMutation({
    mutationFn: async (f: FormState) => {
      // Pre-mutate guard: rooms must have an hourly_rate. Empty/blank silently
      // coerces to 0 via Number(""), which would create a "free" room — almost
      // certainly an admin slip rather than intent. Bail with a thrown Error so
      // the existing onError surfaces it inline.
      if (f.kind === "room" && f.hourly_rate.trim() === "") {
        throw new Error("Hourly rate is required for rooms");
      }
      const billing_config =
        f.kind === "room"
          ? {
              hourly_rate: Number(f.hourly_rate || "0"),
              bucket_minutes: Number(f.bucket_minutes || "1"),
              included_minutes: Number(f.included_minutes || "0"),
              min_charge: Number(f.min_charge || "0"),
            }
          : null;
      const payload = SpotInput.parse({
        name: f.name,
        kind: f.kind,
        billing_config,
        parent_id: f.parent_id.trim() === "" ? null : Number(f.parent_id),
      });
      if (f.id == null) {
        await api.post(`/admin/spots`, Spot, payload);
      } else {
        await api.put(`/admin/spots/${f.id}`, Spot, payload);
      }
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["admin", "spots"] });
      setForm(null);
      setError(null);
    },
    onError: (e: unknown) => setError(e instanceof Error ? e.message : "save failed"),
  });

  const remove = useMutation({
    mutationFn: async (id: number) => {
      await api.delete(`/admin/spots/${id}`, z.void());
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "spots"] }),
  });

  return (
    <div>
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-2xl font-semibold">
          <Trans>Spots</Trans>
        </h1>
        <Button onClick={() => setForm(empty)}>
          <Trans>+ New Spot</Trans>
        </Button>
      </div>
      {list.isLoading && <p>Loading…</p>}
      {list.error && <p className="text-red-600">{String(list.error)}</p>}
      <table className="w-full border-collapse rounded-lg bg-white shadow-sm">
        <thead className="border-b bg-gray-100 text-left text-sm">
          <tr>
            <th className="p-3">ID</th>
            <th className="p-3">
              <Trans>Name</Trans>
            </th>
            <th className="p-3">
              <Trans>Kind</Trans>
            </th>
            <th className="p-3">
              <Trans>Hourly</Trans>
            </th>
            <th className="p-3">
              <Trans>Parent</Trans>
            </th>
            <th className="p-3">
              <Trans>Status</Trans>
            </th>
            <th className="p-3" />
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((s) => (
            <tr key={s.id} className="border-b text-sm last:border-b-0">
              <td className="p-3">{s.id}</td>
              <td className="p-3">{s.name}</td>
              <td className="p-3">{s.kind}</td>
              <td className="p-3">
                {s.billing_config?.hourly_rate?.toLocaleString("vi-VN") ?? "—"}
              </td>
              <td className="p-3">{s.parent_id ?? "—"}</td>
              <td className="p-3">{s.status}</td>
              <td className="p-3 text-right">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setForm({
                      id: s.id,
                      name: s.name,
                      kind: s.kind,
                      hourly_rate:
                        s.billing_config?.hourly_rate?.toString() ?? "",
                      bucket_minutes:
                        s.billing_config?.bucket_minutes?.toString() ?? "1",
                      included_minutes:
                        s.billing_config?.included_minutes?.toString() ?? "0",
                      min_charge:
                        s.billing_config?.min_charge?.toString() ?? "0",
                      parent_id: s.parent_id?.toString() ?? "",
                    })
                  }
                >
                  <Trans>Edit</Trans>
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="text-red-600"
                  onClick={() => {
                    if (window.confirm(`Delete ${s.name}?`)) remove.mutate(s.id);
                  }}
                >
                  <Trans>Delete</Trans>
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <Modal
        open={form != null}
        title={form?.id == null ? t`New spot` : t`Edit spot #${form?.id}`}
        onClose={() => {
          setForm(null);
          setError(null);
        }}
      >
        {form && (
          <form
            className="space-y-3"
            onSubmit={(e) => {
              e.preventDefault();
              upsert.mutate(form);
            }}
          >
            <label className="block text-sm">
              <Trans>Name</Trans>
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                required
              />
            </label>
            <label className="block text-sm">
              <Trans>Kind</Trans>
              <select
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.kind}
                onChange={(e) =>
                  setForm({ ...form, kind: e.target.value as "room" | "table" })
                }
              >
                <option value="table">table</option>
                <option value="room">room</option>
              </select>
            </label>
            {form.kind === "room" && (
              <>
                <label className="block text-sm">
                  <Trans>Hourly rate (VND/h)</Trans>
                  <input
                    className="mt-1 block w-full rounded border px-2 py-1"
                    value={form.hourly_rate}
                    onChange={(e) =>
                      setForm({ ...form, hourly_rate: e.target.value })
                    }
                    type="number"
                    inputMode="numeric"
                    min={0}
                    required
                  />
                </label>
                <label className="block text-sm">
                  <Trans>Bucket (minutes)</Trans>
                  <input
                    className="mt-1 block w-full rounded border px-2 py-1"
                    value={form.bucket_minutes}
                    onChange={(e) =>
                      setForm({ ...form, bucket_minutes: e.target.value })
                    }
                    type="number"
                    inputMode="numeric"
                    min={1}
                    required
                  />
                </label>
                <label className="block text-sm">
                  <Trans>Included minutes</Trans>
                  <input
                    className="mt-1 block w-full rounded border px-2 py-1"
                    value={form.included_minutes}
                    onChange={(e) =>
                      setForm({ ...form, included_minutes: e.target.value })
                    }
                    type="number"
                    inputMode="numeric"
                    min={0}
                  />
                </label>
                <label className="block text-sm">
                  <Trans>Minimum charge (VND)</Trans>
                  <input
                    className="mt-1 block w-full rounded border px-2 py-1"
                    value={form.min_charge}
                    onChange={(e) =>
                      setForm({ ...form, min_charge: e.target.value })
                    }
                    type="number"
                    inputMode="numeric"
                    min={0}
                  />
                </label>
              </>
            )}
            <label className="block text-sm">
              <Trans>Parent ID — leave blank for top-level</Trans>
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.parent_id}
                onChange={(e) => setForm({ ...form, parent_id: e.target.value })}
                inputMode="numeric"
              />
            </label>
            {error && <p className="text-sm text-red-600">{error}</p>}
            <div className="flex justify-end gap-2">
              <Button
                type="button"
                variant="outline"
                onClick={() => {
                  setForm(null);
                  setError(null);
                }}
              >
                <Trans>Cancel</Trans>
              </Button>
              <Button type="submit" disabled={upsert.isPending}>
                <Trans>Save</Trans>
              </Button>
            </div>
          </form>
        )}
      </Modal>
    </div>
  );
}
