import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  useCallback,
} from "react";
import type { ReactNode } from "react";
import { z } from "zod";
import { LoginOutput, TokenClaims } from "@lofi-pos/shared";
import type { TokenClaims as TokenClaimsType } from "@lofi-pos/shared";
import { apiClient, getStoredToken, setStoredToken } from "./api";

interface AuthContextValue {
  token: string | null;
  claims: TokenClaimsType | null;
  login: (pin: string) => Promise<void>;
  logout: () => Promise<void>;
  /** Local lock; clears claims but keeps token (unlock by re-PIN). */
  lock: () => void;
  isAuthenticated: boolean;
  isLocked: boolean;
}

const AuthContext = createContext<AuthContextValue | null>(null);

export function AuthProvider({ children }: { children: ReactNode }) {
  const [token, setToken] = useState<string | null>(getStoredToken());
  const [claims, setClaims] = useState<TokenClaimsType | null>(null);
  const [isLocked, setIsLocked] = useState(false);

  // On mount, if token exists, try /auth/me to recover claims.
  useEffect(() => {
    if (!token) return;
    apiClient
      .get("/auth/me", TokenClaims)
      .then(setClaims)
      .catch(() => {
        // token invalid — clear
        setToken(null);
        setStoredToken(null);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []); // intentionally only on mount

  const login = useCallback(async (pin: string) => {
    const out = await apiClient.post("/auth/login", LoginOutput, { pin });
    setToken(out.token);
    setStoredToken(out.token);
    setClaims(out.claims);
    setIsLocked(false);
  }, []);

  const logout = useCallback(async () => {
    try {
      await apiClient.post("/auth/logout", z.void());
    } catch {
      // ignore — we are tearing down regardless
    }
    setToken(null);
    setClaims(null);
    setStoredToken(null);
    setIsLocked(false);
  }, []);

  const lock = useCallback(() => {
    setIsLocked(true);
    setClaims(null);
  }, []);

  const value = useMemo<AuthContextValue>(
    () => ({
      token,
      claims,
      login,
      logout,
      lock,
      isAuthenticated: !!token && !isLocked,
      isLocked,
    }),
    [token, claims, login, logout, lock, isLocked],
  );

  return <AuthContext.Provider value={value}>{children}</AuthContext.Provider>;
}

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) throw new Error("useAuth outside AuthProvider");
  return ctx;
}
