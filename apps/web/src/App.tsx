import { Navigate, Route, Routes } from "react-router-dom";
import {
  useAuth,
  AppShell,
  LoginRoute,
  LockRoute,
  SessionsRoute,
  SpotPickerRoute,
  SessionDetailRoute,
  PaymentRoute,
} from "@lofi-pos/pos-ui";

export default function App() {
  const { isAuthenticated, isLocked, token } = useAuth();
  if (isLocked || (token && !isAuthenticated)) {
    return (
      <Routes>
        <Route path="*" element={<LockRoute />} />
      </Routes>
    );
  }
  if (!isAuthenticated) {
    return (
      <Routes>
        <Route path="*" element={<LoginRoute />} />
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
