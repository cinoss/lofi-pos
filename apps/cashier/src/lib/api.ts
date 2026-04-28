import { ApiClient, ApiError } from "@lofi-pos/shared";

export const API_BASE = "http://localhost:7878";
export const WS_BASE = "ws://localhost:7878";

export const TOKEN_KEY = "lofi-pos.token";

export function getStoredToken(): string | null {
  return localStorage.getItem(TOKEN_KEY);
}

export function setStoredToken(token: string | null): void {
  if (token === null) localStorage.removeItem(TOKEN_KEY);
  else localStorage.setItem(TOKEN_KEY, token);
}

export const apiClient = new ApiClient({
  baseUrl: API_BASE,
  getToken: getStoredToken,
});

export { ApiError };
