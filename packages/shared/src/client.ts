import { z } from "zod";
import { ApiError, AppErrorEnvelope } from "./error";

export interface ApiClientOptions {
  /** Base URL like http://localhost:7878 (no trailing slash). */
  baseUrl: string;
  /** Returns the current bearer token, or null if not authenticated. */
  getToken: () => string | null;
}

export class ApiClient {
  constructor(private opts: ApiClientOptions) {}

  async request<O extends z.ZodTypeAny>(
    method: "GET" | "POST",
    path: string,
    output: O,
    body?: unknown,
  ): Promise<z.infer<O>> {
    const url = `${this.opts.baseUrl}${path}`;
    const headers: Record<string, string> = {};
    const token = this.opts.getToken();
    if (token) headers["Authorization"] = `Bearer ${token}`;
    if (body !== undefined) headers["Content-Type"] = "application/json";

    const resp = await fetch(url, {
      method,
      headers,
      body: body !== undefined ? JSON.stringify(body) : undefined,
    });

    if (!resp.ok) {
      let envelope: AppErrorEnvelope;
      try {
        const raw = await resp.json();
        envelope = AppErrorEnvelope.parse(raw);
      } catch {
        envelope = { code: "internal", message: resp.statusText };
      }
      throw new ApiError(resp.status, envelope);
    }

    if (resp.status === 204) {
      if (output instanceof z.ZodVoid || output instanceof z.ZodUndefined) {
        return undefined as z.infer<O>;
      }
      throw new Error(
        `Unexpected 204 No Content for non-void response (path=${path}); ` +
          `expected output schema does not accept undefined.`,
      );
    }

    const json = await resp.json();
    return output.parse(json);
  }

  get<O extends z.ZodTypeAny>(path: string, output: O): Promise<z.infer<O>> {
    return this.request("GET", path, output);
  }

  post<O extends z.ZodTypeAny>(
    path: string,
    output: O,
    body?: unknown,
  ): Promise<z.infer<O>> {
    return this.request("POST", path, output, body);
  }
}
