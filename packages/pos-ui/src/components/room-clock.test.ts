import { describe, it, expect } from "vitest";
import { computeRoomCharge, computeRoomChargeBreakdown } from "./room-clock";

const MIN = 60_000;
const billing = (h: number, b: number, inc: number, min: number) => ({
  hourly_rate: h,
  bucket_minutes: b,
  included_minutes: inc,
  min_charge: min,
});

describe("computeRoomCharge", () => {
  it("zero elapsed → 0 when no minimum", () => {
    expect(computeRoomCharge(0, 0, billing(150_000, 1, 0, 0))).toBe(0);
  });

  it("per-minute, no minimum: 5 min @ 150k/h → 12,500", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(150_000, 1, 0, 0))).toBe(12_500);
  });

  it("per-minute, no minimum: 65 min @ 150k/h → 162,500", () => {
    expect(computeRoomCharge(0, 65 * MIN, billing(150_000, 1, 0, 0))).toBe(162_500);
  });

  it("rounds up sub-minute partial: 90 sec @ 60k/h → 1500", () => {
    expect(computeRoomCharge(0, 90_000, billing(60_000, 1, 0, 0))).toBe(2000);
  });

  it("15-min bucket: 5 min @ 60k/h → 15,000 (1 bucket)", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(60_000, 15, 0, 0))).toBe(15_000);
  });

  it("15-min bucket: 16 min @ 60k/h → 30,000 (2 buckets)", () => {
    expect(computeRoomCharge(0, 16 * MIN, billing(60_000, 15, 0, 0))).toBe(30_000);
  });

  it("hour bucket + included = matches old behavior: 5 min @ 150k/h, bucket=60, included=60, min=150k → 150k", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(150_000, 60, 60, 150_000))).toBe(150_000);
  });

  it("hour bucket + included: 65 min @ 150k/h, bucket=60, included=60, min=150k → 300k", () => {
    expect(computeRoomCharge(0, 65 * MIN, billing(150_000, 60, 60, 150_000))).toBe(300_000);
  });

  it("min charge floor: 5 min @ 60k/h, included=30, min=20k → 20k", () => {
    expect(computeRoomCharge(0, 5 * MIN, billing(60_000, 1, 30, 20_000))).toBe(20_000);
  });

  it("included covers exactly: 30 min, included=30, min=20k → 20k", () => {
    expect(computeRoomCharge(0, 30 * MIN, billing(60_000, 1, 30, 20_000))).toBe(20_000);
  });

  it("over-included by 1 min: 31 min, included=30, min=20k → 21k", () => {
    expect(computeRoomCharge(0, 31 * MIN, billing(60_000, 1, 30, 20_000))).toBe(21_000);
  });

  it("over-included by 5 min, 5-min bucket: 35 min, included=30, bucket=5, min=20k → 25k", () => {
    expect(computeRoomCharge(0, 35 * MIN, billing(60_000, 5, 30, 20_000))).toBe(25_000);
  });

  it("over-included by 6 min, 5-min bucket: 36 min → ceil(6/5)=2 buckets * 5 * 60k/60 = 10k overage → 30k total", () => {
    // Plan listed 50k here but the documented formula
    // (overage_buckets * bucket_minutes * hourly_rate / 60) yields 10k overage
    // for 2 buckets at 5 min @ 60k/h, matching the prior 35-min test case's math.
    expect(computeRoomCharge(0, 36 * MIN, billing(60_000, 5, 30, 20_000))).toBe(30_000);
  });

  it("bucket_minutes=0 (degenerate): returns min_charge with no overage", () => {
    // Defensive: malformed JSON shouldn't divide by zero / yield NaN.
    expect(computeRoomCharge(0, 65 * MIN, billing(60_000, 0, 0, 75_000))).toBe(75_000);
    const b = computeRoomChargeBreakdown(0, 65 * MIN, billing(60_000, 0, 0, 75_000));
    expect(b.overageBuckets).toBe(0);
    expect(b.charge).toBe(75_000);
    expect(Number.isFinite(b.charge)).toBe(true);
  });
});

describe("computeRoomChargeBreakdown — display matches charge", () => {
  // Regression: at 90s with 60k/h, the displayed text used to floor elapsedMin
  // to 1 ("1 × 1m × 60k/h") while charge ceiled to 2 buckets (=2,000đ). The
  // breakdown returned must drive both surfaces.
  it("90s @ 60k/h, bucket=1: 2 buckets × 1m, charge 2,000", () => {
    const b = computeRoomChargeBreakdown(0, 90_000, billing(60_000, 1, 0, 0));
    expect(b.elapsedSec).toBe(90);
    expect(b.elapsedMin).toBe(2);
    expect(b.overageMin).toBe(2);
    expect(b.overageBuckets).toBe(2);
    expect(b.charge).toBe(2_000);
    // Multiplied as displayed: buckets × bucket_minutes × hourly_rate / 60.
    expect(b.overageBuckets * 1 * 60_000 / 60).toBe(b.charge);
  });

  it("16 min @ 60k/h, bucket=15: 2 buckets × 15m, charge 30,000", () => {
    const b = computeRoomChargeBreakdown(0, 16 * MIN, billing(60_000, 15, 0, 0));
    expect(b.elapsedMin).toBe(16);
    expect(b.overageBuckets).toBe(2);
    expect(b.charge).toBe(30_000);
    expect(b.overageBuckets * 15 * 60_000 / 60).toBe(b.charge);
  });

  it("inside included period: overageBuckets=0, charge=min_charge", () => {
    const b = computeRoomChargeBreakdown(0, 5 * MIN, billing(150_000, 60, 60, 150_000));
    expect(b.overageBuckets).toBe(0);
    expect(b.overageMin).toBe(0);
    expect(b.charge).toBe(150_000);
  });

  it("elapsedSec uses floor for clock display while elapsedMin uses ceil for billing", () => {
    const b = computeRoomChargeBreakdown(0, 61_500, billing(60_000, 1, 0, 0));
    expect(b.elapsedSec).toBe(61); // 1:01 on the wall clock
    expect(b.elapsedMin).toBe(2); // billed as 2 minutes
  });
});
