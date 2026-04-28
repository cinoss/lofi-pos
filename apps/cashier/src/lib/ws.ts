import type { QueryClient } from "@tanstack/react-query";
import { createEventStream } from "@lofi-pos/shared";
import type { EventNotice } from "@lofi-pos/shared";
import { WS_BASE, getStoredToken } from "./api";

/** Wire WS notices to invalidate the relevant query keys. Returns teardown. */
export function attachWS(queryClient: QueryClient): () => void {
  return createEventStream({
    baseUrl: WS_BASE,
    getToken: getStoredToken,
    onNotice: (n: EventNotice) => {
      // Coarse: any write invalidates active sessions + the touched aggregate
      queryClient.invalidateQueries({ queryKey: ["sessions", "active"] });
      queryClient.invalidateQueries({ queryKey: ["session", n.aggregate_id] });
      queryClient.invalidateQueries({ queryKey: ["order", n.aggregate_id] });
    },
    onError: (e) => console.warn("ws error", e),
  });
}
