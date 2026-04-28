import { useState } from "react";
import { z } from "zod";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { useApiClient } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { Product, ProductInput, ProductKind } from "@lofi-pos/shared";
import type { Route as ProductRoute } from "@lofi-pos/shared";
import { Modal } from "../lib/modal";

const ProductList = z.array(Product);
const ROUTES: ProductRoute[] = ["kitchen", "bar", "none"];
const KINDS = ProductKind.options;

interface FormState {
  id: number | null;
  name: string;
  price: string;
  route: ProductRoute;
  kind: (typeof KINDS)[number];
}
const empty: FormState = {
  id: null,
  name: "",
  price: "0",
  route: "none",
  kind: "item",
};

export function ProductsRoute() {
  const api = useApiClient();
  const qc = useQueryClient();
  const [form, setForm] = useState<FormState | null>(null);
  const [error, setError] = useState<string | null>(null);

  const list = useQuery({
    queryKey: ["admin", "products"],
    queryFn: () => api.get("/admin/products", ProductList),
  });

  const upsert = useMutation({
    mutationFn: async (f: FormState) => {
      const payload = ProductInput.parse({
        name: f.name,
        price: Number(f.price),
        route: f.route,
        kind: f.kind,
      });
      if (f.id == null) {
        await api.post(`/admin/products`, Product, payload);
      } else {
        await api.put(`/admin/products/${f.id}`, Product, payload);
      }
    },
    onSuccess: () => {
      void qc.invalidateQueries({ queryKey: ["admin", "products"] });
      setForm(null);
      setError(null);
    },
    onError: (e: unknown) => setError(e instanceof Error ? e.message : "save failed"),
  });

  const remove = useMutation({
    mutationFn: async (id: number) => api.delete(`/admin/products/${id}`, z.void()),
    onSuccess: () => qc.invalidateQueries({ queryKey: ["admin", "products"] }),
  });

  return (
    <div>
      <div className="mb-4 flex items-center justify-between">
        <h1 className="text-2xl font-semibold">Products</h1>
        <Button onClick={() => setForm(empty)}>+ New Product</Button>
      </div>
      {list.isLoading && <p>Loading…</p>}
      <table className="w-full border-collapse rounded-lg bg-white shadow-sm">
        <thead className="border-b bg-gray-100 text-left text-sm">
          <tr>
            <th className="p-3">ID</th>
            <th className="p-3">Name</th>
            <th className="p-3">Price</th>
            <th className="p-3">Route</th>
            <th className="p-3">Kind</th>
            <th className="p-3" />
          </tr>
        </thead>
        <tbody>
          {(list.data ?? []).map((p) => (
            <tr key={p.id} className="border-b text-sm last:border-b-0">
              <td className="p-3">{p.id}</td>
              <td className="p-3">{p.name}</td>
              <td className="p-3">{p.price}</td>
              <td className="p-3">{p.route}</td>
              <td className="p-3">{p.kind}</td>
              <td className="p-3 text-right">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setForm({
                      id: p.id,
                      name: p.name,
                      price: String(p.price),
                      route: p.route as ProductRoute,
                      kind: p.kind as (typeof KINDS)[number],
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
                    if (window.confirm(`Delete ${p.name}?`)) remove.mutate(p.id);
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
        title={form?.id == null ? "New product" : `Edit product #${form?.id}`}
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
              Price (cents)
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.price}
                onChange={(e) => setForm({ ...form, price: e.target.value })}
                inputMode="numeric"
                required
              />
            </label>
            <label className="block text-sm">
              Route
              <select
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.route}
                onChange={(e) =>
                  setForm({ ...form, route: e.target.value as ProductRoute })
                }
              >
                {ROUTES.map((r) => (
                  <option key={r} value={r}>
                    {r}
                  </option>
                ))}
              </select>
            </label>
            <label className="block text-sm">
              Kind
              <select
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.kind}
                onChange={(e) =>
                  setForm({
                    ...form,
                    kind: e.target.value as (typeof KINDS)[number],
                  })
                }
              >
                {KINDS.map((k) => (
                  <option key={k} value={k}>
                    {k}
                  </option>
                ))}
              </select>
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
