import { createApiClient, getStoredToken } from "@lofi-pos/pos-ui";

// In dev with `pnpm --filter @lofi-pos/admin dev`, the SPA runs on a
// separate Vite port (1421) and talks to the cashier API at 7878. In
// prod, served from the cashier itself so window.location.origin works.
const API_BASE = import.meta.env.VITE_API_BASE ?? window.location.origin;

export const apiClient = createApiClient({
  baseUrl: API_BASE,
  getToken: getStoredToken,
});
export { getStoredToken };
export const WS_BASE = API_BASE.replace(/^http/, "ws");
