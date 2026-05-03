import { Navigate, Route, Routes } from "react-router-dom";
import {
  useAuth,
  AppShell,
  LoginRoute,
  LockRoute,
  SessionsRoute,
  HistoryRoute,
  SpotPickerRoute,
  SessionDetailRoute,
  PaymentRoute,
} from "@lofi-pos/pos-ui";
import { useSetupState } from "./lib/setup";
import { SetupRequiredRoute } from "./routes/setup-required";

export default function App() {
  const setupQ = useSetupState();
  const { isAuthenticated, isLocked, token } = useAuth();

  // Block every other branch until we know whether the venue is configured.
  if (setupQ.isLoading) {
    return (
      <div className="min-h-screen flex items-center justify-center bg-gray-50">
        <p className="text-sm text-gray-500">Loading…</p>
      </div>
    );
  }

  if (setupQ.data?.needs_setup) {
    return <SetupRequiredRoute />;
  }

  // Lock screen wins if either: explicitly locked, or we have a token but
  // /auth/me hasn't (re)hydrated claims yet (or failed). The auth-context
  // clears the token on /auth/me failure, which then drops us into the
  // unauthenticated branch on the next render.
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
      <Route element={<AppShell />}>
        <Route path="/sessions" element={<SessionsRoute />} />
        <Route path="/history" element={<HistoryRoute />} />
        <Route path="/spots" element={<SpotPickerRoute />} />
        <Route path="/sessions/:id" element={<SessionDetailRoute />} />
        <Route path="/sessions/:id/payment" element={<PaymentRoute />} />
        <Route path="*" element={<Navigate to="/sessions" replace />} />
      </Route>
    </Routes>
  );
}
