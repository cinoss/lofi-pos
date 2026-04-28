import { useState } from "react";
import { useNavigate } from "react-router-dom";
import { ApiError } from "@lofi-pos/shared";
import { PinPad } from "../components/pin-pad";
import { useAuth } from "../auth-context";

export function LoginRoute() {
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
      if (e instanceof ApiError) {
        if (e.code === "rate_limited")
          setError("Too many attempts. Try again in a minute.");
        else if (e.isUnauthorized()) setError("Wrong PIN");
        else setError(e.message);
      } else {
        setError("Network error");
      }
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex min-h-screen items-center justify-center bg-gray-50">
      <PinPad
        title="LoFi POS — Sign In"
        onSubmit={onSubmit}
        error={error}
        disabled={busy}
      />
    </div>
  );
}
