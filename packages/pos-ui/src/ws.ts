import type { QueryClient } from "@tanstack/react-query";
import { createEventStream } from "@lofi-pos/shared";
import type { EventNotice } from "@lofi-pos/shared";

export interface WSConfig {
  baseUrl: string;
  getToken: () => string | null;
}

export function attachWS(config: WSConfig, queryClient: QueryClient): () => void {
  return createEventStream({
    baseUrl: config.baseUrl,
    getToken: config.getToken,
    onNotice: (n: EventNotice) => {
      queryClient.invalidateQueries({ queryKey: ["sessions", "active"] });
      queryClient.invalidateQueries({ queryKey: ["session", n.aggregate_id] });
      queryClient.invalidateQueries({ queryKey: ["order", n.aggregate_id] });
    },
    onError: (e) => console.warn("ws error", e),
  });
}
