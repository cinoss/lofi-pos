import { EventNotice } from "./schemas/common";
import type { EventNotice as EventNoticeT } from "./schemas/common";

export interface EventStreamOptions {
  /** Base URL like ws://localhost:7878 (no trailing slash). */
  baseUrl: string;
  getToken: () => string | null;
  onNotice: (notice: EventNoticeT) => void;
  onError?: (err: Error) => void;
  onOpen?: () => void;
  onClose?: () => void;
}

/**
 * Connects to /ws, performs the {type:"hello", token} handshake, then
 * dispatches EventNotice payloads to onNotice. Returns a teardown function
 * that closes the underlying WebSocket.
 */
export function createEventStream(opts: EventStreamOptions): () => void {
  const url = `${opts.baseUrl}/ws`;
  const ws = new WebSocket(url);

  ws.addEventListener("open", () => {
    const token = opts.getToken();
    if (!token) {
      ws.close(4001, "no token");
      return;
    }
    ws.send(JSON.stringify({ type: "hello", token }));
  });

  ws.addEventListener("message", (ev: MessageEvent) => {
    try {
      const raw = typeof ev.data === "string" ? ev.data : "";
      const data = JSON.parse(raw) as { type?: string; code?: string };
      if (data && data.type === "hello_ok") {
        opts.onOpen?.();
        return;
      }
      if (data && data.type === "error") {
        opts.onError?.(new Error(`ws error: ${data.code ?? "unknown"}`));
        return;
      }
      // Otherwise expect an EventNotice
      const notice = EventNotice.parse(data);
      opts.onNotice(notice);
    } catch (e) {
      opts.onError?.(e as Error);
    }
  });

  ws.addEventListener("close", () => opts.onClose?.());
  ws.addEventListener("error", () =>
    opts.onError?.(new Error("ws transport error")),
  );

  return () => ws.close();
}
