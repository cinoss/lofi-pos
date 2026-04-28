import { useState } from "react";
import { Button } from "@lofi-pos/ui/components/button";

interface PinPadProps {
  onSubmit: (pin: string) => Promise<void> | void;
  /** Show error message above the pad (e.g., "wrong PIN"). */
  error?: string | null;
  disabled?: boolean;
  title?: string;
}

const KEYS: Array<string | "back" | "submit"> = [
  "1",
  "2",
  "3",
  "4",
  "5",
  "6",
  "7",
  "8",
  "9",
  "back",
  "0",
  "submit",
];

export function PinPad({
  onSubmit,
  error,
  disabled,
  title = "Enter PIN",
}: PinPadProps) {
  const [pin, setPin] = useState("");

  const press = (k: string) => {
    if (disabled) return;
    if (k === "back") setPin((p) => p.slice(0, -1));
    else if (k === "submit") {
      void onSubmit(pin);
    } else if (pin.length < 12) setPin((p) => p + k);
  };

  return (
    <div className="flex flex-col items-center gap-4 p-6">
      <h2 className="text-2xl font-semibold">{title}</h2>
      <div className="text-3xl tracking-[0.5em] font-mono h-12">
        {pin.replace(/./g, "•") || (
          <span className="text-gray-400">______</span>
        )}
      </div>
      {error && <div className="text-red-600 text-sm">{error}</div>}
      <div className="grid grid-cols-3 gap-3">
        {KEYS.map((k) => (
          <Button
            key={k}
            variant={
              k === "submit" ? "default" : k === "back" ? "outline" : "secondary"
            }
            size="lg"
            className="h-16 w-20 text-xl"
            onClick={() => press(k)}
            disabled={disabled || (k === "submit" && pin.length < 6)}
          >
            {k === "back" ? "⌫" : k === "submit" ? "✓" : k}
          </Button>
        ))}
      </div>
    </div>
  );
}
