import { createApiClient, getStoredToken } from "@lofi-pos/pos-ui";

const API_BASE = import.meta.env.VITE_API_BASE ?? "http://localhost:7878";
export const apiClient = createApiClient({ baseUrl: API_BASE, getToken: getStoredToken });
export { getStoredToken };
export const WS_BASE = API_BASE.replace(/^http/, "ws");
