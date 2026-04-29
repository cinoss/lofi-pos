import { z } from "zod";
import { Role, Route, SpotKind } from "./common";

// ----------- Spots -----------
export const SpotInput = z
  .object({
    name: z.string().min(1),
    kind: SpotKind,
    hourly_rate: z.number().int().nullable(),
    parent_id: z.number().int().nullable(),
  })
  .strict();
export type SpotInput = z.infer<typeof SpotInput>;

// ----------- Staff -----------
export const StaffInput = z
  .object({
    name: z.string().min(1),
    pin: z.string().min(6),
    role: Role,
    team: z.string().nullable(),
  })
  .strict();
export type StaffInput = z.infer<typeof StaffInput>;

// PATCH-style update — every field optional. `team`'s tri-state encoding:
//   undefined → leave alone, null → clear, string → set.
export const StaffUpdate = z
  .object({
    name: z.string().min(1).optional(),
    pin: z.string().min(6).optional(),
    role: Role.optional(),
    team: z.string().nullable().optional(),
  })
  .strict();
export type StaffUpdate = z.infer<typeof StaffUpdate>;

// ----------- Products -----------
export const ProductKind = z.enum(["item", "recipe", "time"]);
export type ProductKind = z.infer<typeof ProductKind>;

export const ProductInput = z
  .object({
    name: z.string().min(1),
    price: z.number().int().nonnegative(),
    route: Route,
    kind: ProductKind,
  })
  .strict();
export type ProductInput = z.infer<typeof ProductInput>;

// ----------- Settings (admin update payload) -----------
export const SettingsUpdate = z
  .object({
    business_day_cutoff_hour: z.number().int().min(0).max(23).optional(),
    business_day_tz_offset_seconds: z.number().int().optional(),
    discount_threshold_pct: z.number().int().optional(),
    cancel_grace_minutes: z.number().int().optional(),
    idle_lock_minutes: z.number().int().optional(),
  })
  .strict();
export type SettingsUpdate = z.infer<typeof SettingsUpdate>;

