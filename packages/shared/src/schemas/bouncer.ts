import { z } from "zod";

/** A seed advertised by the bouncer sidecar. */
export const BouncerSeed = z
  .object({
    id: z.string(),
    label: z.string(),
    default: z.boolean(),
    seed_hex: z.string().regex(/^[0-9a-f]{64}$/),
  })
  .strict();
export type BouncerSeed = z.infer<typeof BouncerSeed>;

export const BouncerSeedList = z.array(BouncerSeed);
export type BouncerSeedList = z.infer<typeof BouncerSeedList>;
