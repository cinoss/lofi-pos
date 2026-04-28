import { Navigate, Route, Routes } from "react-router-dom";
import { useAuth, AppShell } from "@lofi-pos/pos-ui";
import { LoginRoute } from "./routes/login";
import { LockRoute } from "./routes/lock";
import { SessionsRoute } from "./routes/sessions";
import { SpotPickerRoute } from "./routes/spot-picker";
import { SessionDetailRoute } from "./routes/session-detail";
import { PaymentRoute } from "./routes/payment";

export default function App() {
  const { isAuthenticated, isLocked, token } = useAuth();

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
        <Route path="/spots" element={<SpotPickerRoute />} />
        <Route path="/sessions/:id" element={<SessionDetailRoute />} />
        <Route path="/sessions/:id/payment" element={<PaymentRoute />} />
        <Route path="*" element={<Navigate to="/sessions" replace />} />
      </Route>
    </Routes>
  );
}
