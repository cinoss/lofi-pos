import { z } from "zod";

export const Settings = z.object({
  business_day_cutoff_hour: z.number().int(),
  business_day_tz_offset_seconds: z.number().int(),
  discount_threshold_pct: z.number().int(),
  cancel_grace_minutes: z.number().int(),
  idle_lock_minutes: z.number().int(),
}).strict();
export type Settings = z.infer<typeof Settings>;
