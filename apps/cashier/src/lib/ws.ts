import { attachWS as attachWSFactory } from "@lofi-pos/pos-ui";
import type { QueryClient } from "@tanstack/react-query";
import { WS_BASE, getStoredToken } from "./api";

export function attachWS(qc: QueryClient): () => void {
  return attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);
}
