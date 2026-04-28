import { createApiClient, getStoredToken, setStoredToken, TOKEN_KEY } from "@lofi-pos/pos-ui";
import { ApiError } from "@lofi-pos/shared";

const API_BASE = import.meta.env.VITE_API_BASE ?? "http://localhost:7878";
export const apiClient = createApiClient({ baseUrl: API_BASE, getToken: getStoredToken });
export { getStoredToken, setStoredToken, TOKEN_KEY, ApiError, API_BASE };
export const WS_BASE = API_BASE.replace(/^http/, "ws");
