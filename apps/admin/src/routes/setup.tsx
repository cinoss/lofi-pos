import { useState } from "react";
import { useMutation } from "@tanstack/react-query";
import { Button } from "@lofi-pos/ui/components/button";
import { SetupRequest, ApiError, AppErrorEnvelope } from "@lofi-pos/shared";

const API_BASE = import.meta.env.VITE_API_BASE ?? window.location.origin;

interface FormState {
  // Venue
  venue_name: string;
  venue_address: string;
  venue_phone: string;
  currency: string;
  locale: string;
  tax_id: string;
  receipt_footer: string;
  // Operational
  business_day_cutoff_hour: string;
  business_day_tz_offset_seconds: string;
  // Owner
  owner_name: string;
  owner_pin: string;
  owner_pin_confirm: string;
}

const DEFAULTS: FormState = {
  venue_name: "",
  venue_address: "",
  venue_phone: "",
  currency: "VND",
  locale: "vi-VN",
  tax_id: "",
  receipt_footer: "",
  business_day_cutoff_hour: "11",
  business_day_tz_offset_seconds: "25200",
  owner_name: "",
  owner_pin: "",
  owner_pin_confirm: "",
};

interface SetupRouteProps {
  /** Called after a successful setup so the parent can re-probe setup state. */
  onSuccess?: () => void;
}

export function SetupRoute({ onSuccess }: SetupRouteProps) {
  const [form, setForm] = useState<FormState>(DEFAULTS);
  const [error, setError] = useState<string | null>(null);
  const [success, setSuccess] = useState(false);

  const submit = useMutation({
    mutationFn: async (f: FormState) => {
      if (f.owner_pin !== f.owner_pin_confirm) {
        throw new Error("PIN confirmation does not match");
      }
      const payload = SetupRequest.parse({
        venue_name: f.venue_name.trim(),
        venue_address: f.venue_address,
        venue_phone: f.venue_phone,
        currency: f.currency.trim(),
        locale: f.locale.trim(),
        tax_id: f.tax_id,
        receipt_footer: f.receipt_footer,
        business_day_cutoff_hour: Number(f.business_day_cutoff_hour),
        business_day_tz_offset_seconds: Number(f.business_day_tz_offset_seconds),
        owner_name: f.owner_name.trim(),
        owner_pin: f.owner_pin,
      });
      // POST /admin/setup returns 201 with no body. The shared ApiClient
      // doesn't currently support 201-empty, so issue the request directly.
      const url = `${API_BASE}/admin/setup`;
      const resp = await fetch(url, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify(payload),
      });
      if (!resp.ok) {
        let envelope: AppErrorEnvelope;
        try {
          envelope = AppErrorEnvelope.parse(await resp.json());
        } catch {
          envelope = { code: "internal", message: resp.statusText };
        }
        throw new ApiError(resp.status, envelope);
      }
    },
    onSuccess: () => {
      setError(null);
      setSuccess(true);
    },
    onError: (e: unknown) => {
      setSuccess(false);
      setError(e instanceof Error ? e.message : "Setup failed");
    },
  });

  const update = (key: keyof FormState) => (e: React.ChangeEvent<HTMLInputElement | HTMLTextAreaElement>) =>
    setForm({ ...form, [key]: e.target.value });

  if (success) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50 p-6">
        <div className="max-w-lg w-full rounded-lg bg-white p-8 shadow-sm space-y-4 text-center">
          <h1 className="text-2xl font-semibold text-green-700">Setup complete</h1>
          <p className="text-gray-700">
            Your venue is configured and the Owner account has been created.
          </p>
          <p className="text-gray-700">
            Return to the cashier and click <strong>I&apos;ve finished setup</strong> to
            continue. You can now also sign in to this admin console with the Owner
            PIN you just set.
          </p>
          <Button onClick={() => (onSuccess ? onSuccess() : window.location.reload())}>
            Continue
          </Button>
        </div>
      </div>
    );
  }

  return (
    <div className="min-h-screen bg-gray-50 p-6">
      <div className="mx-auto max-w-2xl space-y-6">
        <header className="space-y-1">
          <h1 className="text-3xl font-semibold">First-time setup</h1>
          <p className="text-gray-600 text-sm">
            Welcome. Configure your venue, operational defaults, and the Owner
            account. All fields are saved together; nothing is persisted until
            you submit successfully.
          </p>
        </header>

        <form
          className="space-y-6"
          onSubmit={(e) => {
            e.preventDefault();
            setError(null);
            submit.mutate(form);
          }}
        >
          <section className="rounded-lg bg-white p-6 shadow-sm space-y-3">
            <h2 className="text-lg font-semibold">Venue identity</h2>
            <Field label="Venue name *">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.venue_name}
                onChange={update("venue_name")}
                required
              />
            </Field>
            <Field label="Address">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.venue_address}
                onChange={update("venue_address")}
              />
            </Field>
            <Field label="Phone">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.venue_phone}
                onChange={update("venue_phone")}
              />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Currency *">
                <input
                  className="mt-1 block w-full rounded border px-2 py-1"
                  value={form.currency}
                  onChange={update("currency")}
                  required
                />
              </Field>
              <Field label="Locale *">
                <input
                  className="mt-1 block w-full rounded border px-2 py-1"
                  value={form.locale}
                  onChange={update("locale")}
                  required
                />
              </Field>
            </div>
            <Field label="Tax ID">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.tax_id}
                onChange={update("tax_id")}
              />
            </Field>
            <Field label="Receipt footer">
              <textarea
                className="mt-1 block w-full rounded border px-2 py-1"
                rows={2}
                value={form.receipt_footer}
                onChange={update("receipt_footer")}
              />
            </Field>
          </section>

          <section className="rounded-lg bg-white p-6 shadow-sm space-y-3">
            <h2 className="text-lg font-semibold">Operational</h2>
            <Field label="Business-day cutoff hour (0–23)">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                inputMode="numeric"
                value={form.business_day_cutoff_hour}
                onChange={update("business_day_cutoff_hour")}
                required
              />
            </Field>
            <Field label="Timezone offset (seconds, e.g. 25200 = +07:00)">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                inputMode="numeric"
                value={form.business_day_tz_offset_seconds}
                onChange={update("business_day_tz_offset_seconds")}
                required
              />
            </Field>
          </section>

          <section className="rounded-lg bg-white p-6 shadow-sm space-y-3">
            <h2 className="text-lg font-semibold">Owner account</h2>
            <p className="text-sm text-gray-600">
              The Owner can sign into the cashier and the admin console. Choose
              a PIN of at least 6 digits and keep it private.
            </p>
            <Field label="Owner name *">
              <input
                className="mt-1 block w-full rounded border px-2 py-1"
                value={form.owner_name}
                onChange={update("owner_name")}
                required
              />
            </Field>
            <div className="grid grid-cols-2 gap-3">
              <Field label="Owner PIN (min 6) *">
                <input
                  className="mt-1 block w-full rounded border px-2 py-1"
                  type="password"
                  inputMode="numeric"
                  minLength={6}
                  value={form.owner_pin}
                  onChange={update("owner_pin")}
                  required
                />
              </Field>
              <Field label="Confirm PIN *">
                <input
                  className="mt-1 block w-full rounded border px-2 py-1"
                  type="password"
                  inputMode="numeric"
                  minLength={6}
                  value={form.owner_pin_confirm}
                  onChange={update("owner_pin_confirm")}
                  required
                />
              </Field>
            </div>
          </section>

          {error && (
            <p className="text-sm text-red-600 rounded bg-red-50 p-3">{error}</p>
          )}
          <div className="flex justify-end">
            <Button type="submit" disabled={submit.isPending}>
              {submit.isPending ? "Saving…" : "Complete setup"}
            </Button>
          </div>
        </form>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block text-sm">
      <span className="font-medium">{label}</span>
      {children}
    </label>
  );
}
