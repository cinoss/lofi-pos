import { z } from "zod";

export const Settings = z
  .object({
    // Operational
    business_day_cutoff_hour: z.number().int().min(0).max(23),
    business_day_tz_offset_seconds: z.number().int(),
    discount_threshold_pct: z.number().int().nonnegative(),
    cancel_grace_minutes: z.number().int().nonnegative(),
    idle_lock_minutes: z.number().int().positive(),
    // Venue
    venue_name: z.string(),
    venue_address: z.string(),
    venue_phone: z.string(),
    currency: z.string(),
    locale: z.string(),
    tax_id: z.string(),
    receipt_footer: z.string(),
  })
  .strict();
export type Settings = z.infer<typeof Settings>;

export const SettingsUpdate = Settings.partial().strict();
export type SettingsUpdate = z.infer<typeof SettingsUpdate>;
