import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { t } from "@lingui/core/macro";
import { ApiError } from "@lofi-pos/shared";
import { PinPad } from "../components/pin-pad";
import { useAuth } from "../auth-context";

/**
 * Lock screen — shown after idle timeout. PIN entry re-runs login (fresh
 * token, fresh jti). The previous token is left orphaned in localStorage
 * until re-login overwrites it.
 */
export function LockRoute() {
  const { login } = useAuth();
  const nav = useNavigate();
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const onSubmit = async (pin: string) => {
    setBusy(true);
    setError(null);
    try {
      await login(pin);
      nav("/sessions", { replace: true });
    } catch (e) {
      if (e instanceof ApiError && e.isUnauthorized()) setError(t`Wrong PIN`);
      else setError(e instanceof Error ? e.message : t`Error`);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-gray-900">
      <div className="rounded-lg bg-white p-2">
        <PinPad
          title={t`🔒 Locked — Re-enter PIN`}
          onSubmit={onSubmit}
          error={error}
          disabled={busy}
        />
      </div>
    </div>
  );
}
