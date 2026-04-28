import { z } from "zod";

export const PaymentState = z.object({
  session_id: z.string(),
  subtotal: z.number().int(),
  discount_pct: z.number().int(),
  vat_pct: z.number().int(),
  total: z.number().int(),
  method: z.string(),
}).strict();
export type PaymentState = z.infer<typeof PaymentState>;

export const TakePaymentInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  subtotal: z.number().int(),
  discount_pct: z.number().int(),
  vat_pct: z.number().int(),
  total: z.number().int(),
  method: z.string(),
}).strict();
export type TakePaymentInput = z.infer<typeof TakePaymentInput>;
