import { useQuery, useMutation, useQueryClient } from "@tanstack/react-query";
import { useNavigate, useParams } from "react-router-dom";
import { z } from "zod";
import { useState } from "react";
import { SessionState, OrderState, Product } from "@lofi-pos/shared";
import type {
  PlaceOrderInput,
  RawOrderItem,
  CloseSessionInput,
} from "@lofi-pos/shared";
import { apiClient, ApiError } from "../lib/api";
import { Button } from "@lofi-pos/ui/components/button";

function newKey(): string {
  return crypto.randomUUID();
}

export function SessionDetailRoute() {
  const { id } = useParams<{ id: string }>();
  const sessionId = id!;
  const qc = useQueryClient();
  const nav = useNavigate();

  const { data: session } = useQuery({
    queryKey: ["session", sessionId],
    queryFn: () => apiClient.get(`/sessions/${sessionId}`, SessionState),
  });

  const { data: products } = useQuery({
    queryKey: ["products"],
    queryFn: () => apiClient.get("/products", z.array(Product)),
  });

  const placeOrder = useMutation({
    mutationFn: (items: RawOrderItem[]) => {
      const input: PlaceOrderInput = {
        idempotency_key: newKey(),
        session_id: sessionId,
        items,
      };
      return apiClient.post("/orders", OrderState, input);
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["session", sessionId] });
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      setCart({});
    },
  });

  const closeSession = useMutation({
    mutationFn: () => {
      const input: CloseSessionInput = { idempotency_key: newKey() };
      return apiClient.post(
        `/sessions/${sessionId}/close`,
        SessionState,
        input,
      );
    },
    onSuccess: () => {
      qc.invalidateQueries({ queryKey: ["sessions", "active"] });
      nav("/sessions");
    },
  });

  // Cart state: product_id -> qty
  const [cart, setCart] = useState<Record<number, number>>({});
  const cartItems: RawOrderItem[] = Object.entries(cart)
    .filter(([, qty]) => qty > 0)
    .map(([pid, qty]) => ({ product_id: Number(pid), qty }));

  if (!session) return <div>Loading…</div>;

  return (
    <div className="grid grid-cols-1 lg:grid-cols-2 gap-6">
      <section>
        <h1 className="text-2xl font-semibold mb-2">{session.spot.name}</h1>
        <div className="text-sm text-gray-500 mb-4">
          {session.spot.kind} · {session.order_ids.length} orders
        </div>

        <div className="rounded-lg border bg-white p-4 mb-4">
          <h2 className="font-semibold mb-2">Order list</h2>
          {session.order_ids.length === 0 ? (
            <p className="text-gray-500 text-sm">No orders placed yet.</p>
          ) : (
            <ul className="text-sm text-gray-700">
              {session.order_ids.map((oid) => (
                <li key={oid}>· {oid.slice(0, 8)}…</li>
              ))}
            </ul>
          )}
        </div>

        <div className="flex gap-2">
          <Button onClick={() => nav(`/sessions/${sessionId}/payment`)}>
            Take payment
          </Button>
          <Button
            variant="outline"
            onClick={() => closeSession.mutate()}
            disabled={closeSession.isPending}
          >
            Close session
          </Button>
        </div>
        {closeSession.error instanceof ApiError && (
          <div className="text-red-600 text-sm mt-2">
            {closeSession.error.message}
          </div>
        )}
      </section>

      <section>
        <h2 className="text-xl font-semibold mb-3">Add items</h2>
        <div className="grid grid-cols-2 gap-2 mb-4">
          {products?.map((p) => (
            <div
              key={p.id}
              className="rounded border bg-white p-3 flex items-center justify-between"
            >
              <div>
                <div className="font-medium">{p.name}</div>
                <div className="text-xs text-gray-500">
                  {p.price.toLocaleString("vi-VN")}đ
                </div>
              </div>
              <div className="flex items-center gap-2">
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setCart((c) => ({
                      ...c,
                      [p.id]: Math.max(0, (c[p.id] ?? 0) - 1),
                    }))
                  }
                >
                  −
                </Button>
                <span className="w-8 text-center">{cart[p.id] ?? 0}</span>
                <Button
                  size="sm"
                  variant="outline"
                  onClick={() =>
                    setCart((c) => ({ ...c, [p.id]: (c[p.id] ?? 0) + 1 }))
                  }
                >
                  +
                </Button>
              </div>
            </div>
          ))}
        </div>
        <Button
          className="w-full"
          disabled={cartItems.length === 0 || placeOrder.isPending}
          onClick={() => placeOrder.mutate(cartItems)}
        >
          Place order ({cartItems.length})
        </Button>
        {placeOrder.error instanceof ApiError && (
          <div className="text-red-600 text-sm mt-2">
            {placeOrder.error.message}
          </div>
        )}
      </section>
    </div>
  );
}
