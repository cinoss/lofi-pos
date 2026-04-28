import { useState } from "react";
import { z } from "zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiClient } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { StaffInput, StaffOut } from "@lofi-pos/shared";
import type { Role } from "@lofi-pos/shared";
import { Modal } from "../lib/modal";

const StaffList = z.array(StaffOut);
const ROLES: Role[] = ["staff", "cashier", "manager", "owner"];

interface FormState {
  id: number | null;
  name: string;
  pin: string; // empty on edit = leave alone
  role: Role;
  team: string;
}
const empty: FormState = { id: null, name: "", pin: "", role: "staff", team: "" };

export function StaffRoute() {
  const api = useApiClient();
  const qc = useQueryClient();
  const [form, setForm] = useState<FormState | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: ["admin", "staff"],
    queryFn: () => api.get("/admin/staff", StaffList),
  });

  const upsert = useMutation({
    mutationFn: async (f: FormState) => {
      if (f.id == null) {
        const payload = StaffInput.parse({
          name: f.name,
          pin: f.pin,
          role: f.role,
          team: f.team.trim() === "" ? null : f.team,
        });
        await api.post(`/admin/staff`, StaffOut, payload);
      } else {
        const body: Record<string, unknown> = {
          name: f.name,
          role: f.role,
          team: f.team.trim() === "" ? null : f.team,
        };
        if (f.pin.trim() !== "") body.pin = f.pin;
        await api.put(`/admin/staff/${f.id}`, StaffOut, body);
      }
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["admin", "staff"] });
      setForm(null);
      setError(null);
    },
    onError: (e: unknown) => setError(e instanceof Error ? e.message : "save failed"),
  });

  const remove = useMutation({
    mutationFn: async (id: number) => api.delete(`/admin/staff/${id}`, z.void()),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "staff"] }),
  });

  return (
    <div>
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Staff</h1>
        <Button onClick={() => setForm(empty)}>+ New Staff</Button>
      </div>
      {list.isLoading && <p>Loading…</p>}
      <table className="w-full border-collapse rounded-lg bg-white shadow-sm">
        <thead className="border-b bg-gray-100 text-left text-sm">
          <tr>
            <th className="p-3">ID</th>
            <th className="p-3">Name</th>
            <th className="p-3">Role</th>
            <th className="p-3">Team</th>
            <th className="p-3" />
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((s) => (
            <tr key={s.id} className="border-b text-sm last:border-b-0">
              <td className="p-3">{s.id}</td>
              <td className="p-3">{s.name}</td>
              <td className="p-3">{s.role}</td>
              <td className="p-3">{s.team ?? "—"}</td>
              <td className="p-3 text-right">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setForm({
                      id: s.id,
                      name: s.name,
                      pin: "",
                      role: s.role,
                      team: s.team ?? "",
                    })
                  }
                >
                  Edit
                </Button>
                <Button
                  size="sm"
                  variant="ghost"
                  className="text-red-600"
                  onClick={() => {
                    if (window.confirm(`Delete ${s.name}?`)) remove.mutate(s.id);
                  }}
                >
                  Delete
                </Button>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <Modal
        open={form != null}
        title={form?.id == null ? "New staff" : `Edit staff #${form?.id}`}
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
              Name
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.name}
                onChange={(e) => setForm({ ...form, name: e.target.value })}
                required
              />
            </label>
            <label className="block text-sm">
              PIN {form.id != null && "(leave blank to keep current)"}
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.pin}
                onChange={(e) => setForm({ ...form, pin: e.target.value })}
                type="password"
                autoComplete="new-password"
                inputMode="numeric"
                minLength={form.id == null ? 6 : 0}
                required={form.id == null}
              />
            </label>
            <label className="block text-sm">
              Role
              <select
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.role}
                onChange={(e) => setForm({ ...form, role: e.target.value as Role })}
              >
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
            </label>
            <label className="block text-sm">
              Team — blank for none
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.team}
                onChange={(e) => setForm({ ...form, team: e.target.value })}
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
                Cancel
              </Button>
              <Button type="submit" disabled={upsert.isPending}>
                Save
              </Button>
            </div>
          </form>
        )}
      </Modal>
    </div>
  );
}
