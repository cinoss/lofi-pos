import { Link, Outlet } from "react-router-dom";
import { Button } from "@lofi-pos/ui/components/button";
import { useAuth } from "../lib/auth-context";
import { useSettings } from "../lib/settings-context";
import { useIdleTimer } from "../lib/idle-tracker";

export function AppShell() {
  const { claims, lock, logout } = useAuth();
  const settings = useSettings();
  const idleMs = (settings?.idle_lock_minutes ?? 10) * 60 * 1000;
  useIdleTimer(idleMs, lock);

  return (
    <div className="min-h-screen flex flex-col">
      <header className="flex items-center justify-between border-b bg-white px-6 py-3">
        <Link to="/sessions" className="text-xl font-semibold">
          LoFi POS
        </Link>
        <nav className="flex items-center gap-4">
          <Link to="/sessions" className="text-sm hover:underline">
            Sessions
          </Link>
          <Link to="/spots" className="text-sm hover:underline">
            Open New
          </Link>
          {claims && (
            <span className="text-sm text-gray-500">
              {claims.role} · staff #{claims.staff_id}
            </span>
          )}
          <Button size="sm" variant="outline" onClick={lock}>
            Lock
          </Button>
          <Button size="sm" variant="ghost" onClick={() => void logout()}>
            Logout
          </Button>
        </nav>
      </header>
      <main className="flex-1 bg-gray-50 p-6">
        <Outlet />
      </main>
    </div>
  );
}
