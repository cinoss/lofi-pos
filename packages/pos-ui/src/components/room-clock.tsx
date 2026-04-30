import { useEffect, useState } from "react";
import { Trans } from "@lingui/react/macro";

interface RoomClockProps {
  /** Wall-clock ms when the SessionOpened event was appended (from SessionState.opened_at_ms). */
  openedAtMs: number;
  /** Per-hour billing rate in VND, from SpotRef.Room.hourly_rate. */
  hourlyRate: number;
  /** Optional refresh cadence; defaults to 30s — minute-level resolution is plenty for a wall clock. */
  refreshMs?: number;
}

/**
 * Live-updating elapsed-time + projected room charge for a time-billed (room)
 * session. The display is informational only; the authoritative time-charge
 * is computed at payment time via `computeRoomCharge` and folded into the
 * subtotal prefill (see payment.tsx).
 *
 * Billing rule: ceil(elapsed minutes / 60), with a 1-hour minimum. Vietnamese
 * karaoke bars typically charge by the hour with the partial hour rounded up
 * — never sub-hour billing — so a 5-minute session still costs one full hour.
 */
export function RoomClock({
  openedAtMs,
  hourlyRate,
  refreshMs = 30_000,
}: RoomClockProps) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), refreshMs);
    return () => clearInterval(t);
  }, [refreshMs]);

  const elapsedMs = Math.max(0, now - openedAtMs);
  const elapsedMin = Math.floor(elapsedMs / 60_000);
  const hh = Math.floor(elapsedMin / 60);
  const mm = elapsedMin % 60;

  const billableHours = Math.max(1, Math.ceil(elapsedMin / 60));
  const charge = billableHours * hourlyRate;

  return (
    <div className="rounded border bg-blue-50 p-3 mb-2">
      <div className="text-xs text-gray-600">
        <Trans>Room time</Trans>
      </div>
      <div className="text-lg font-mono">
        {hh.toString().padStart(2, "0")}:{mm.toString().padStart(2, "0")}
      </div>
      <div className="text-sm text-gray-700">
        {billableHours}h × {hourlyRate.toLocaleString("vi-VN")}đ ={" "}
        <span className="font-semibold">
          {charge.toLocaleString("vi-VN")}đ
        </span>
      </div>
    </div>
  );
}

/**
 * Pure helper used by both the running clock and payment prefill so the two
 * surfaces always agree on the projected charge.
 *
 * Returns the billable VND charge for `(nowMs - openedAtMs)` against the
 * given hourly rate. Same rounding rule as `RoomClock`: ceil to next full
 * hour, minimum one hour.
 */
export function computeRoomCharge(
  openedAtMs: number,
  nowMs: number,
  hourlyRate: number,
): number {
  const elapsedMin = Math.max(0, Math.floor((nowMs - openedAtMs) / 60_000));
  const billableHours = Math.max(1, Math.ceil(elapsedMin / 60));
  return billableHours * hourlyRate;
}
