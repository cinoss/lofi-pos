import { z } from "zod";
import { Role, SpotKind } from "./common";

export const Spot = z.object({
  id: z.number().int(),
  name: z.string(),
  kind: SpotKind,
  hourly_rate: z.number().int().nullable(),
  parent_id: z.number().int().nullable(),
  status: z.string(),
}).strict();
export type Spot = z.infer<typeof Spot>;

export const Product = z.object({
  id: z.number().int(),
  name: z.string(),
  price: z.number().int(),
  route: z.string(),
  kind: z.string(),
}).strict();
export type Product = z.infer<typeof Product>;

export const StaffOut = z.object({
  id: z.number().int(),
  name: z.string(),
  role: Role,
  team: z.string().nullable(),
}).strict();
export type StaffOut = z.infer<typeof StaffOut>;
