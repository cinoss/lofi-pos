import { useEffect, useState } from "react";
import { useMutation, useQuery, useQueries } from "@tanstack/react-query";
import { useNavigate, useParams } from "react-router-dom";
import { Trans } from "@lingui/react/macro";
import { t } from "@lingui/core/macro";
import { SessionState, OrderState, ApiError } from "@lofi-pos/shared";
import type { TakePaymentInput } from "@lofi-pos/shared";
import { Button } from "@lofi-pos/ui/components/button";
import { OverrideModal } from "../components/override-modal";
import { computeRoomCharge } from "../components/room-clock";
import { Breadcrumbs } from "../components/breadcrumbs";
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

  // For time-billed (room) sessions, compute the projected room-time charge
  // once at payment-screen load. Time charge is informational here — it gets
  // folded into the subtotal prefill so the cashier sees the correct number,
  // but it is not yet emitted as a separate line item (deferred until a
  // dedicated `product.kind='time'` integration). Recomputing on every render
  // would make the field unstable, so we capture it once.
  const roomCharge =
    session?.spot.kind === "room" && session.opened_at_ms
      ? computeRoomCharge(
          session.opened_at_ms,
          Date.now(),
          session.spot.billing,
        )
      : 0;

  const [subtotal, setSubtotal] = useState(0);
  // Default subtotal once the live computed value lands. Don't overwrite if
  // the cashier has already typed a value.
  useEffect(() => {
    if (subtotal === 0 && (computedSubtotal > 0 || roomCharge > 0)) {
      setSubtotal(computedSubtotal + roomCharge);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [computedSubtotal, roomCharge]);

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
      <Breadcrumbs
        items={[
          { label: <Trans>Sessions</Trans>, to: "/sessions" },
          {
            label: session?.spot.name ?? "…",
            to: `/sessions/${sessionId}`,
          },
          { label: <Trans>Payment</Trans> },
        ]}
      />
      <h1 className="text-2xl font-semibold mb-4">
        <Trans>Take payment</Trans>
      </h1>
      <div className="space-y-3">
        <Field
          label={t`Subtotal (VND)`}
          value={subtotal}
          onChange={setSubtotal}
          min={0}
        />
        {roomCharge > 0 && (
          <div className="text-sm text-gray-600 -mt-2">
            <Trans>Includes room time charge:</Trans>{" "}
            {roomCharge.toLocaleString("vi-VN")}đ
          </div>
        )}
        <Field
          label={t`Discount %`}
          value={discountPct}
          onChange={setDiscountPct}
          min={0}
          max={100}
        />
        <Field
          label={t`VAT %`}
          value={vatPct}
          onChange={setVatPct}
          min={0}
          max={100}
        />
        <div>
          <label className="block text-sm mb-1">
            <Trans>Method</Trans>
          </label>
          <select
            className="border rounded px-2 py-1 w-full"
            value={method}
            onChange={(e) => setMethod(e.target.value)}
          >
            <option value="cash">{t`Cash`}</option>
            <option value="card">{t`Card`}</option>
            <option value="transfer">{t`Bank transfer`}</option>
          </select>
        </div>
        <div className="text-2xl font-bold pt-2 border-t">
          <Trans>Total: {total.toLocaleString("vi-VN")}đ</Trans>
        </div>
        <Button
          className="w-full"
          disabled={
            pay.isPending ||
            session?.payment_taken ||
            session?.status !== "Open" ||
            (subtotal === 0 && (session?.order_ids.length ?? 0) === 0)
          }
          onClick={submit}
        >
          <Trans>Charge</Trans>
        </Button>
        {session?.payment_taken && (
          <div className="text-amber-700 text-sm">
            <Trans>This session has already been paid.</Trans>
          </div>
        )}
        {!session?.payment_taken &&
          subtotal === 0 &&
          (session?.order_ids.length ?? 0) === 0 && (
            <div className="text-gray-600 text-sm">
              <Trans>
                Add an order or enter a non-zero subtotal before charging.
              </Trans>
            </div>
          )}
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
  min,
  max,
}: {
  label: string;
  value: number;
  onChange: (n: number) => void;
  min?: number;
  max?: number;
}) {
  return (
    <div>
      <label className="block text-sm mb-1">{label}</label>
      <input
        type="number"
        className="border rounded px-2 py-1 w-full"
        value={value}
        min={min}
        max={max}
        onChange={(e) => {
          let n = Number(e.target.value);
          if (Number.isNaN(n)) n = 0;
          if (typeof min === "number" && n < min) n = min;
          if (typeof max === "number" && n > max) n = max;
          onChange(n);
        }}
      />
    </div>
  );
}
