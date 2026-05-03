import { useEffect, useState } from "react";
import { Trans } from "@lingui/react/macro";

/**
 * Snapshot of a room's billing policy as captured into SpotRef::Room at
 * session-open / transfer time. All four fields are required.
 *
 * Fields:
 * - `hourly_rate`: VND per hour for overage billing
 * - `bucket_minutes`: billing granularity for overage (e.g. 1 = per minute, 60 = per hour)
 * - `included_minutes`: minutes covered by `min_charge` with no extra cost
 * - `min_charge`: VND minimum charge that covers the included period
 */
export interface RoomBilling {
  hourly_rate: number;
  bucket_minutes: number;
  included_minutes: number;
  min_charge: number;
}

interface RoomClockProps {
  /** Wall-clock ms when the SessionOpened event was appended (from SessionState.opened_at_ms). */
  openedAtMs: number;
  /** Snapshotted billing policy from SpotRef.Room.billing. */
  billing: RoomBilling;
  /** Optional refresh cadence; defaults to 1s for HH:MM:SS display. */
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
  billing,
  refreshMs = 1_000,
}: RoomClockProps) {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), refreshMs);
    return () => clearInterval(t);
  }, [refreshMs]);

  const elapsedMs = Math.max(0, now - openedAtMs);
  const totalSec = Math.floor(elapsedMs / 1000);
  const hh = Math.floor(totalSec / 3600);
  const mm = Math.floor((totalSec % 3600) / 60);
  const ss = totalSec % 60;
  const elapsedMin = Math.floor(elapsedMs / 60_000);

  const charge = computeRoomCharge(openedAtMs, now, billing);
  const inIncluded = elapsedMin <= billing.included_minutes;
  const overageMin = Math.max(0, elapsedMin - billing.included_minutes);
  const overageBuckets =
    billing.bucket_minutes > 0
      ? Math.ceil(overageMin / billing.bucket_minutes)
      : 0;

  return (
    <div className="rounded border bg-blue-50 p-3 mb-2">
      <div className="text-xs text-gray-600">
        <Trans>Room time</Trans>
      </div>
      <div className="text-lg font-mono">
        {hh.toString().padStart(2, "0")}:{mm.toString().padStart(2, "0")}:
        {ss.toString().padStart(2, "0")}
      </div>
      <div className="text-sm text-gray-700">
        {inIncluded && billing.included_minutes > 0 ? (
          <Trans>Included period: {billing.min_charge.toLocaleString("vi-VN")}đ</Trans>
        ) : (
          <>
            {billing.min_charge > 0 && (
              <>{billing.min_charge.toLocaleString("vi-VN")}đ + </>
            )}
            {overageBuckets} × {billing.bucket_minutes}m ×{" "}
            {billing.hourly_rate.toLocaleString("vi-VN")}đ/h ={" "}
            <span className="font-semibold">
              {charge.toLocaleString("vi-VN")}đ
            </span>
          </>
        )}
      </div>
    </div>
  );
}

/**
 * Pure helper used by both the running clock and payment prefill so the two
 * surfaces always agree on the projected charge.
 *
 * Billing rule:
 * - elapsed_min ≤ included_minutes → charge = min_charge
 * - else → charge = min_charge + ceil(overage_buckets * bucket_minutes * hourly_rate / 60)
 */
export function computeRoomCharge(
  openedAtMs: number,
  nowMs: number,
  billing: RoomBilling,
): number {
  const elapsedMs = Math.max(0, nowMs - openedAtMs);
  // Round elapsed up to the next whole minute so a 90-second session bills
  // against 2 minutes rather than 1 — matches the bucket model where partial
  // units always round up.
  const elapsedMin = Math.ceil(elapsedMs / 60_000);
  if (elapsedMin <= billing.included_minutes) {
    return billing.min_charge;
  }
  const overageMin = elapsedMin - billing.included_minutes;
  const overageBuckets = Math.ceil(overageMin / billing.bucket_minutes);
  const overageCharge = Math.ceil(
    (overageBuckets * billing.bucket_minutes * billing.hourly_rate) / 60,
  );
  return billing.min_charge + overageCharge;
}
