import { Link, Outlet } from "react-router-dom";
import { Trans } from "@lingui/react/macro";
import { Button } from "@lofi-pos/ui/components/button";
import { useAuth } from "../auth-context";
import { useSettings } from "../settings-context";
import { useIdleTimer } from "../idle-tracker";
import { useApiClient } from "../api-context";
import { ConnectionStatus } from "./connection-status";

export function AppShell() {
  const { claims, lock, logout } = useAuth();
  const apiClient = useApiClient();
  const settings = useSettings();
  const idleMs = (settings?.idle_lock_minutes ?? 10) * 60 * 1000;
  useIdleTimer(idleMs, lock);

  // Admin SPA is served by the cashier axum at /ui/admin/ (proxied to vite
  // in dev). Use the api client's baseUrl rather than window.location.origin
  // — under Tauri dev, location.origin is the cashier vite (:1420), but
  // /ui/admin/ lives on the cashier axum (:7878 / VITE_API_BASE).
  const adminUrl = `${apiClient.baseUrl}/ui/admin/`;

  // Tauri webview swallows <a target="_blank"> by default — clicking the
  // link does nothing. Use the opener plugin to surface the OS browser
  // when available, fall back to window.open for plain web (tablet) builds.
  // Resolved via a string variable so neither vite nor tsc try to type-check
  // the optional @tauri-apps/plugin-opener dep (pos-ui doesn't depend on it
  // directly; cashier installs it, web app doesn't need it).
  const handleAdminClick = async (e: React.MouseEvent<HTMLAnchorElement>) => {
    e.preventDefault();
    const isTauri =
      typeof window !== "undefined" &&
      ("__TAURI_INTERNALS__" in window || "__TAURI__" in window);
    if (isTauri) {
      try {
        const moduleName = "@tauri-apps/plugin-opener";
        const mod = (await import(/* @vite-ignore */ moduleName)) as {
          openUrl: (u: string) => Promise<void>;
        };
        await mod.openUrl(adminUrl);
        return;
      } catch (err) {
        console.warn("tauri opener unavailable, falling back", err);
      }
    }
    window.open(adminUrl, "_blank", "noopener");
  };

  return (
    <div className="min-h-screen flex flex-col">
      <header className="flex items-center justify-between border-b bg-white px-6 py-3">
        <Link to="/sessions" className="text-xl font-semibold">
          LoFi POS
        </Link>
        <nav className="flex items-center gap-4">
          <Link to="/sessions" className="text-sm hover:underline">
            <Trans>Sessions</Trans>
          </Link>
          <Link to="/spots" className="text-sm hover:underline">
            <Trans>Open New</Trans>
          </Link>
          <Link to="/history" className="text-sm hover:underline">
            <Trans>History</Trans>
          </Link>
          {claims?.role === "owner" && (
            <a
              href={adminUrl}
              target="_blank"
              rel="noreferrer"
              onClick={(e) => void handleAdminClick(e)}
              className="text-sm hover:underline"
            >
              <Trans>Admin</Trans>
            </a>
          )}
          <ConnectionStatus />
          {claims && (
            <span className="text-sm text-gray-500">
              <Trans>
                {claims.role} · staff #{claims.staff_id}
              </Trans>
            </span>
          )}
          <Button size="sm" variant="outline" onClick={lock}>
            <Trans>Lock</Trans>
          </Button>
          <Button size="sm" variant="ghost" onClick={() => void logout()}>
            <Trans>Logout</Trans>
          </Button>
        </nav>
      </header>
      <main className="flex-1 bg-gray-50 p-6">
        <Outlet />
      </main>
    </div>
  );
}
