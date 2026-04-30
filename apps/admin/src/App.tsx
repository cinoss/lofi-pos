import { Navigate, Route, Routes, Link, Outlet } from "react-router-dom";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Trans } from "@lingui/react/macro";
import { useAuth, LoginRoute, LockRoute, useApiClient } from "@lofi-pos/pos-ui";
import { Button } from "@lofi-pos/ui/components/button";
import { SetupState } from "@lofi-pos/shared";
import { SpotsRoute } from "./routes/spots";
import { StaffRoute } from "./routes/staff";
import { ProductsRoute } from "./routes/products";
import { SettingsRoute } from "./routes/settings";
import { ReportsRoute } from "./routes/reports";
import { SetupRoute } from "./routes/setup";

function AdminShell() {
  const { claims, logout } = useAuth();
  if (claims?.role !== "owner") {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <div className="bg-white rounded-lg p-8 shadow-md max-w-md text-center">
          <h1 className="text-xl font-semibold mb-2">
            <Trans>Owner role required</Trans>
          </h1>
          <p className="text-sm text-gray-600 mb-4">
            <Trans>
              The admin console is restricted to the Owner role. Your account is
              currently {claims?.role ?? "unauthenticated"}.
            </Trans>
          </p>
          <Button variant="outline" onClick={() => void logout()}>
            <Trans>Logout</Trans>
          </Button>
        </div>
      </div>
    );
  }
  return (
    <div className="min-h-screen flex flex-col">
      <header className="flex items-center justify-between border-b bg-white px-6 py-3">
        <Link to="/spots" className="text-xl font-semibold">
          <Trans>LoFi POS — Admin</Trans>
        </Link>
        <nav className="flex items-center gap-4 text-sm">
          <Link to="/spots" className="hover:underline">
            <Trans>Spots</Trans>
          </Link>
          <Link to="/staff" className="hover:underline">
            <Trans>Staff</Trans>
          </Link>
          <Link to="/products" className="hover:underline">
            <Trans>Products</Trans>
          </Link>
          <Link to="/settings" className="hover:underline">
            <Trans>Settings</Trans>
          </Link>
          <Link to="/reports" className="hover:underline">
            <Trans>Reports</Trans>
          </Link>
          <span className="text-gray-500">
            <Trans>
              {claims.role} · staff #{claims.staff_id}
            </Trans>
          </span>
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

export default function App() {
  const api = useApiClient();
  const qc = useQueryClient();
  const setupQ = useQuery({
    queryKey: ["setup-state"],
    queryFn: () => api.get("/admin/setup-state", SetupState),
    refetchOnWindowFocus: true,
  });
  const { isAuthenticated, isLocked, token } = useAuth();

  if (setupQ.isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <p className="text-sm text-gray-500"><Trans>Loading…</Trans></p>
      </div>
    );
  }

  if (setupQ.data?.needs_setup) {
    return (
      <SetupRoute
        onSuccess={() => {
          void qc.invalidateQueries({ queryKey: ["setup-state"] });
        }}
      />
    );
  }

  if (isLocked || (token && !isAuthenticated)) {
    return (
      <Routes>
        <Route path="/lock" element={<LockRoute />} />
        <Route path="*" element={<Navigate to="/lock" replace />} />
      </Routes>
    );
  }

  if (!isAuthenticated) {
    return (
      <Routes>
        <Route path="/login" element={<LoginRoute />} />
        <Route path="*" element={<Navigate to="/login" replace />} />
      </Routes>
    );
  }

  return (
    <Routes>
      <Route element={<AdminShell />}>
        <Route path="/spots" element={<SpotsRoute />} />
        <Route path="/staff" element={<StaffRoute />} />
        <Route path="/products" element={<ProductsRoute />} />
        <Route path="/settings" element={<SettingsRoute />} />
        <Route path="/reports" element={<ReportsRoute />} />
        <Route path="*" element={<Navigate to="/spots" replace />} />
      </Route>
    </Routes>
  );
}
