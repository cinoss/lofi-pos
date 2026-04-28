import { useEffect, useState } from "react";
import { Button } from "@lofi-pos/ui/components/button";
import { MIN_PIN_LEN } from "@lofi-pos/shared";

interface PinPadProps {
  onSubmit: (pin: string) => Promise<void> | void;
  /** Show error message above the pad (e.g., "wrong PIN"). */
  error?: string | null;
  disabled?: boolean;
  title?: string;
  /**
   * Optional override. If omitted, auto-detects via `(pointer: coarse)`
   * media query — touch on tablets/phones, default on desktops.
   */
  size?: "default" | "touch";
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
  size,
}: PinPadProps) {
  const [pin, setPin] = useState("");
  const [autoTouch, setAutoTouch] = useState(false);

  useEffect(() => {
    if (typeof window === "undefined" || !window.matchMedia) return;
    const mq = window.matchMedia("(pointer: coarse)");
    const update = () => setAutoTouch(mq.matches);
    update();
    mq.addEventListener("change", update);
    return () => mq.removeEventListener("change", update);
  }, []);

  const effectiveSize: "default" | "touch" =
    size ?? (autoTouch ? "touch" : "default");

  const press = (k: string) => {
    if (disabled) return;
    if (k === "back") setPin((p) => p.slice(0, -1));
    else if (k === "submit") {
      void onSubmit(pin);
    } else if (pin.length < 12) setPin((p) => p + k);
  };

  const buttonClass =
    effectiveSize === "touch" ? "h-24 w-28 text-2xl" : "h-16 w-20 text-xl";
  const titleClass = effectiveSize === "touch" ? "text-3xl" : "text-2xl";

  return (
    <div className="flex flex-col items-center gap-4 p-6">
      <h2 className={`${titleClass} font-semibold`}>{title}</h2>
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
            className={buttonClass}
            onClick={() => press(k)}
            disabled={disabled || (k === "submit" && pin.length < MIN_PIN_LEN)}
          >
            {k === "back" ? "⌫" : k === "submit" ? "✓" : k}
          </Button>
        ))}
      </div>
    </div>
  );
}
