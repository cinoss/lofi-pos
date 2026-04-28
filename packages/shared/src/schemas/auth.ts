import { z } from "zod";
import { Role } from "./common";

export const MIN_PIN_LEN = 6;
export const LoginInput = z.object({ pin: z.string().min(MIN_PIN_LEN) }).strict();
export type LoginInput = z.infer<typeof LoginInput>;

export const TokenClaims = z.object({
  staff_id: z.number().int(),
  role: Role,
  iat: z.number().int(),
  exp: z.number().int(),
  jti: z.string(),
}).strict();
export type TokenClaims = z.infer<typeof TokenClaims>;

export const LoginOutput = z.object({
  token: z.string(),
  claims: TokenClaims,
}).strict();
export type LoginOutput = z.infer<typeof LoginOutput>;
