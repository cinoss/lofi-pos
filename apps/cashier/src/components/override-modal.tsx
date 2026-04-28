import { useState } from "react";
import { PinPad } from "./pin-pad";
import { Button } from "@lofi-pos/ui/components/button";

interface Props {
  /** Role required to authorize, e.g., "manager". Shown in title. */
  minRole: string;
  open: boolean;
  onSubmit: (pin: string) => Promise<void> | void;
  onCancel: () => void;
}

export function OverrideModal({ minRole, open, onSubmit, onCancel }: Props) {
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  if (!open) return null;

  const handle = async (pin: string) => {
    setBusy(true);
    setError(null);
    try {
      await onSubmit(pin);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Override rejected");
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="fixed inset-0 bg-black/50 flex items-center justify-center z-50">
      <div className="bg-white rounded-lg p-4 max-w-sm">
        <PinPad
          title={`${minRole} override`}
          onSubmit={handle}
          error={error}
          disabled={busy}
        />
        <Button
          variant="outline"
          className="w-full mt-2"
          onClick={onCancel}
          disabled={busy}
        >
          Cancel
        </Button>
      </div>
    </div>
  );
}
