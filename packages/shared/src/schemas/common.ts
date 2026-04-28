import { z } from "zod";

export const Role = z.enum(["staff", "cashier", "manager", "owner"]);
export type Role = z.infer<typeof Role>;

export const Route = z.enum(["kitchen", "bar", "none"]);
export type Route = z.infer<typeof Route>;

export const SpotKind = z.enum(["room", "table"]);
export type SpotKind = z.infer<typeof SpotKind>;

export const SpotRef = z.discriminatedUnion("kind", [
  z.object({
    kind: z.literal("room"),
    id: z.number().int(),
    name: z.string(),
    hourly_rate: z.number().int(),
  }).strict(),
  z.object({
    kind: z.literal("table"),
    id: z.number().int(),
    name: z.string(),
    room_id: z.number().int().nullable(),
    room_name: z.string().nullable(),
  }).strict(),
]);
export type SpotRef = z.infer<typeof SpotRef>;

export const RecipeIngredientSnapshot = z.object({
  ingredient_id: z.number().int(),
  ingredient_name: z.string(),
  qty: z.number(),
  unit: z.string(),
}).strict();
export type RecipeIngredientSnapshot = z.infer<typeof RecipeIngredientSnapshot>;

export const OrderItemSpec = z.object({
  product_id: z.number().int(),
  product_name: z.string(),
  qty: z.number().int(),
  unit_price: z.number().int(),
  note: z.string().nullable(),
  route: Route,
  recipe_snapshot: z.array(RecipeIngredientSnapshot),
}).strict();
export type OrderItemSpec = z.infer<typeof OrderItemSpec>;

export const NoticeKind = z.enum(["event_appended"]);
export type NoticeKind = z.infer<typeof NoticeKind>;

export const EventNotice = z.object({
  kind: NoticeKind,
  event_type: z.string(),
  aggregate_id: z.string(),
  ts: z.number().int(),
}).strict();
export type EventNotice = z.infer<typeof EventNotice>;
