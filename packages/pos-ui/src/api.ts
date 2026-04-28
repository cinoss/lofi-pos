import { ApiClient } from "@lofi-pos/shared";

export interface ApiClientConfig {
  baseUrl: string;
  getToken: () => string | null;
}

export function createApiClient(config: ApiClientConfig): ApiClient {
  return new ApiClient(config);
}

export const TOKEN_KEY = "lofi-pos.token";

export function getStoredToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}
export function setStoredToken(token: string | null): void {
  if (token === null) localStorage.removeItem(TOKEN_KEY);
  else localStorage.setItem(TOKEN_KEY, token);
}
