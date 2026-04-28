import { z } from "zod";
import { Role } from "./common";

export const LoginInput = z.object({ pin: z.string().min(6) }).strict();
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
