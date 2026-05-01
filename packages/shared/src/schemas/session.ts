import { z } from "zod";
import { SpotRef } from "./common";

export const SessionStatus = z.union([
  z.literal("Open"),
  z.literal("Closed"),
  z.object({ Merged: z.object({ into: z.string() }).strict() }).strict(),
  z.literal("Split"),
]);
export type SessionStatus = z.infer<typeof SessionStatus>;

export const SessionState = z.object({
  session_id: z.string(),
  status: SessionStatus,
  spot: SpotRef,
  opened_by: z.number().int(),
  opened_at_ms: z.number().int(),
  customer_label: z.string().nullable(),
  team: z.string().nullable(),
  order_ids: z.array(z.string()),
  /** True once a PaymentTaken event has been applied for this session.
   *  UI hides cancel/return controls when set. */
  payment_taken: z.boolean(),
}).strict();
export type SessionState = z.infer<typeof SessionState>;

export const OpenSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  spot_id: z.number().int(),
  customer_label: z.string().nullable().optional(),
  team: z.string().nullable().optional(),
}).strict();
export type OpenSessionInput = z.infer<typeof OpenSessionInput>;

export const CloseSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  reason: z.string().nullable().optional(),
}).strict();
export type CloseSessionInput = z.infer<typeof CloseSessionInput>;

export const TransferSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  to_spot_id: z.number().int(),
}).strict();
export type TransferSessionInput = z.infer<typeof TransferSessionInput>;

export const MergeSessionsInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  into_session: z.string(),
  sources: z.array(z.string()),
}).strict();
export type MergeSessionsInput = z.infer<typeof MergeSessionsInput>;

export const SplitSessionInput = z.object({
  idempotency_key: z.string(),
  override_pin: z.string().nullable().optional(),
  new_sessions: z.array(z.string()),
}).strict();
export type SplitSessionInput = z.infer<typeof SplitSessionInput>;
