import { z } from "zod";

export const SetupState = z
  .object({
    needs_setup: z.boolean(),
    /** LAN-reachable URL of this cashier (e.g. http://192.168.1.45:7878),
     *  or http://localhost:7878 if no LAN IP could be resolved. */
    lan_url: z.string().url(),
  })
  .strict();
export type SetupState = z.infer<typeof SetupState>;

export const SetupRequest = z
  .object({
    // Venue
    venue_name: z.string().min(1),
    venue_address: z.string(),
    venue_phone: z.string(),
    currency: z.string().min(1),
    locale: z.string().min(1),
    tax_id: z.string(),
    receipt_footer: z.string(),
    // Operational
    business_day_cutoff_hour: z.number().int().min(0).max(23),
    business_day_tz_offset_seconds: z.number().int(),
    // Owner account
    owner_name: z.string().min(1),
    owner_pin: z.string().min(6),
  })
  .strict();
export type SetupRequest = z.infer<typeof SetupRequest>;
