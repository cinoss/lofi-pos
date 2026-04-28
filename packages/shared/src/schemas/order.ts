import { z } from "zod";
import { OrderItemSpec } from "./common";

export const OrderItem = z.object({
  spec: OrderItemSpec,
  cancelled: z.boolean(),
  returned_qty: z.number().int(),
}).strict();
export type OrderItem = z.infer<typeof OrderItem>;

export const OrderState = z.object({
  order_id: z.string(),
  session_id: z.string(),
  items: z.array(OrderItem),
}).strict();
export type OrderState = z.infer<typeof OrderState>;

export const RawOrderItem = z.object({
  product_id: z.number().int(),
  qty: z.number().int(),
  note: z.string().nullable().optional(),
}).strict();
export type RawOrderItem = z.infer<typeof RawOrderItem>;

export const PlaceOrderInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  session_id: z.string(),
  items: z.array(RawOrderItem),
}).strict();
export type PlaceOrderInput = z.infer<typeof PlaceOrderInput>;

export const CancelOrderItemInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  reason: z.string().nullable().optional(),
  is_self: z.boolean(),
  within_grace: z.boolean(),
}).strict();
export type CancelOrderItemInput = z.infer<typeof CancelOrderItemInput>;

export const ReturnOrderItemInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  qty: z.number().int(),
  reason: z.string().nullable().optional(),
}).strict();
export type ReturnOrderItemInput = z.infer<typeof ReturnOrderItemInput>;
