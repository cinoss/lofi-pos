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

  const { elapsedSec, elapsedMin, overageBuckets, charge } =
    computeRoomChargeBreakdown(openedAtMs, now, billing);
  const hh = Math.floor(elapsedSec / 3600);
  const mm = Math.floor((elapsedSec % 3600) / 60);
  const ss = elapsedSec % 60;
  const inIncluded = elapsedMin <= billing.included_minutes;

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

export interface RoomChargeBreakdown {
  /** Whole seconds since open (floor), suitable for HH:MM:SS display. */
  elapsedSec: number;
  /** Whole minutes since open, rounded UP — matches the bucket model. */
  elapsedMin: number;
  /** Minutes past `included_minutes`, before bucket rounding (>= 0). */
  overageMin: number;
  /** Number of overage buckets billed (`ceil(overageMin / bucket_minutes)`). */
  overageBuckets: number;
  /** Final VND charge — `min_charge` plus overage. */
  charge: number;
}

/**
 * Pure helper used by both the running clock and payment prefill so the two
 * surfaces always agree on the projected charge AND the breakdown shown to
 * the user. Returning a single struct prevents the display text from drifting
 * away from the charge total (was a bug when display used floor and the
 * charge used ceil — at 90s, display said "1×1m" but charged for 2).
 *
 * Billing rule:
 * - elapsed_min ≤ included_minutes → charge = min_charge
 * - else → charge = min_charge + ceil(overage_buckets * bucket_minutes * hourly_rate / 60)
 *
 * Defensive: if `bucket_minutes <= 0` (malformed JSON), returns `min_charge`
 * with zero overage rather than NaN/Infinity.
 */
export function computeRoomChargeBreakdown(
  openedAtMs: number,
  nowMs: number,
  billing: RoomBilling,
): RoomChargeBreakdown {
  const elapsedMs = Math.max(0, nowMs - openedAtMs);
  const elapsedSec = Math.floor(elapsedMs / 1000);
  // Round elapsed up to the next whole minute so a 90-second session bills
  // against 2 minutes rather than 1 — matches the bucket model where partial
  // units always round up.
  const elapsedMin = Math.ceil(elapsedMs / 60_000);
  // Degenerate config (bucket=0) → no overage charging; return min_charge.
  if (billing.bucket_minutes <= 0) {
    return {
      elapsedSec,
      elapsedMin,
      overageMin: 0,
      overageBuckets: 0,
      charge: billing.min_charge,
    };
  }
  if (elapsedMin <= billing.included_minutes) {
    return {
      elapsedSec,
      elapsedMin,
      overageMin: 0,
      overageBuckets: 0,
      charge: billing.min_charge,
    };
  }
  const overageMin = elapsedMin - billing.included_minutes;
  const overageBuckets = Math.ceil(overageMin / billing.bucket_minutes);
  const overageCharge = Math.ceil(
    (overageBuckets * billing.bucket_minutes * billing.hourly_rate) / 60,
  );
  return {
    elapsedSec,
    elapsedMin,
    overageMin,
    overageBuckets,
    charge: billing.min_charge + overageCharge,
  };
}

/**
 * Convenience wrapper preserving the original numeric-only API for callers
 * that only care about the final amount (e.g. payment prefill).
 */
export function computeRoomCharge(
  openedAtMs: number,
  nowMs: number,
  billing: RoomBilling,
): number {
  return computeRoomChargeBreakdown(openedAtMs, nowMs, billing).charge;
}
