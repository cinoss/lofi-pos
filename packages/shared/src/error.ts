import { z } from "zod";

export const AppErrorEnvelope = z.object({
  code: z.string(),
  message: z.string().optional(),
}).strict();
export type AppErrorEnvelope = z.infer<typeof AppErrorEnvelope>;

/** Thrown by ApiClient on non-2xx responses; carries the parsed envelope. */
export class ApiError extends Error {
  readonly code: string;
  readonly status: number;
  readonly envelope: AppErrorEnvelope;

  constructor(status: number, envelope: AppErrorEnvelope) {
    super(envelope.message ?? envelope.code);
    this.code = envelope.code;
    this.status = status;
    this.envelope = envelope;
  }

  isOverrideRequired(): boolean {
    return this.code === "override_required";
  }
  isUnauthorized(): boolean {
    return this.code === "unauthorized";
  }
  isValidation(): boolean {
    return this.code === "validation";
  }
  isConflict(): boolean {
    return this.code === "conflict";
  }
}
