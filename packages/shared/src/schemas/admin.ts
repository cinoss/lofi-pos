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
// `SettingsUpdate` is exported from ./settings.ts (derived from the canonical
// Settings schema via `Settings.partial().strict()`).

