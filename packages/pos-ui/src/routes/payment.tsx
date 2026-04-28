import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueries } from "@tanstack/react-query";
import { useNavigate, useParams } from "react-router-dom";
import { SessionState, OrderState, ApiError } from "@lofi-pos/shared";
import type { TakePaymentInput } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { OverrideModal } from "../components/override-modal";
import { useApiClient } from "../api-context";

export function PaymentRoute() {
  const apiClient = useApiClient();
  const { id } = useParams<{ id: string }>();
  const sessionId = id!;
  const nav = useNavigate();

  const { data: session } = useQuery({
    queryKey: ["session", sessionId],
    queryFn: () => apiClient.get(`/sessions/${sessionId}`, SessionState),
  });

  const orderQueries = useQueries({
    queries: (session?.order_ids ?? []).map((oid) => ({
      queryKey: ["order", oid],
      queryFn: () => apiClient.get(`/orders/${oid}`, OrderState),
      enabled: !!session,
    })),
  });

  const orders = orderQueries
    .map((q) => q.data)
    .filter((o): o is NonNullable<typeof o> => !!o);

  const computedSubtotal = orders.reduce((sum, o) => {
    return (
      sum +
      o.items.reduce((s, it) => {
        if (it.cancelled) return s;
        const netQty = Math.max(0, it.spec.qty - it.returned_qty);
        return s + netQty * it.spec.unit_price;
      }, 0)
    );
  }, 0);

  const [subtotal, setSubtotal] = useState(0);
  // Default subtotal once the live computed value lands. Don't overwrite if
  // the cashier has already typed a value.
  useEffect(() => {
    if (subtotal === 0 && computedSubtotal > 0) setSubtotal(computedSubtotal);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [computedSubtotal]);

  const [discountPct, setDiscountPct] = useState(0);
  const [vatPct, setVatPct] = useState(8);
  const [method, setMethod] = useState("cash");

  // Stable idempotency key per form intent.
  const [idempotencyKey] = useState(() => crypto.randomUUID());

  const total = Math.round(
    (((subtotal * (100 - discountPct)) / 100) * (100 + vatPct)) / 100,
  );

  const [pendingInput, setPendingInput] = useState<TakePaymentInput | null>(
    null,
  );
  const [overrideRole, setOverrideRole] = useState<string | null>(null);

  const pay = useMutation({
    mutationFn: (input: TakePaymentInput) =>
      apiClient.post(`/sessions/${sessionId}/payment`, SessionState, input),
    onSuccess: () => nav("/sessions"),
    onError: (e: unknown) => {
      if (e instanceof ApiError && e.code === "override_required") {
        setOverrideRole(e.envelope.message ?? "manager");
      }
    },
  });

  const submit = () => {
    const input: TakePaymentInput = {
      idempotency_key: idempotencyKey,
      subtotal,
      discount_pct: discountPct,
      vat_pct: vatPct,
      total,
      method,
    };
    setPendingInput(input);
    pay.mutate(input);
  };

  const submitWithOverride = async (overridePin: string) => {
    if (!pendingInput) return;
    await pay.mutateAsync({ ...pendingInput, override_pin: overridePin });
    setOverrideRole(null);
  };

  return (
    <div className="max-w-md mx-auto">
      <h1 className="text-2xl font-semibold mb-4">Take payment</h1>
      <div className="space-y-3">
        <Field label="Subtotal (VND)" value={subtotal} onChange={setSubtotal} />
        <Field
          label="Discount %"
          value={discountPct}
          onChange={setDiscountPct}
        />
        <Field label="VAT %" value={vatPct} onChange={setVatPct} />
        <div>
          <label className="block text-sm mb-1">Method</label>
          <select
            className="border rounded px-2 py-1 w-full"
            value={method}
            onChange={(e) => setMethod(e.target.value)}
          >
            <option value="cash">Cash</option>
            <option value="card">Card</option>
            <option value="transfer">Bank transfer</option>
          </select>
        </div>
        <div className="text-2xl font-bold pt-2 border-t">
          Total: {total.toLocaleString("vi-VN")}đ
        </div>
        <Button
          className="w-full"
          disabled={pay.isPending}
          onClick={submit}
        >
          Charge
        </Button>
        {pay.error instanceof ApiError &&
          pay.error.code !== "override_required" && (
            <div className="text-red-600 text-sm">{pay.error.message}</div>
          )}
      </div>
      <OverrideModal
        open={overrideRole !== null}
        minRole={overrideRole ?? "manager"}
        onSubmit={submitWithOverride}
        onCancel={() => setOverrideRole(null)}
      />
    </div>
  );
}

function Field({
  label,
  value,
  onChange,
}: {
  label: string;
  value: number;
  onChange: (n: number) => void;
}) {
  return (
    <div>
      <label className="block text-sm mb-1">{label}</label>
      <input
        type="number"
        className="border rounded px-2 py-1 w-full"
        value={value}
        onChange={(e) => onChange(Number(e.target.value))}
      />
    </div>
  );
}
