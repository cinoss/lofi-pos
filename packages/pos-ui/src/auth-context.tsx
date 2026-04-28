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
import { useQueryClient } from "@tanstack/react-query";
import type { QueryClient } from "@tanstack/react-query";
import type { ApiClient } from "@lofi-pos/shared";
import { LoginOutput, TokenClaims } from "@lofi-pos/shared";
import type { TokenClaims as TokenClaimsType } from "@lofi-pos/shared";
import { setStoredToken, getStoredToken } from "./api";

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

export interface AuthProviderProps {
  client: ApiClient;
  attachWS: (qc: QueryClient) => () => void;
  children: ReactNode;
}

export function AuthProvider({ client, attachWS, children }: AuthProviderProps) {
  const [token, setToken] = useState<string | null>(getStoredToken());
  const [claims, setClaims] = useState<TokenClaimsType | null>(null);
  const [isLocked, setIsLocked] = useState(false);
  const qc = useQueryClient();

  // Attach WS whenever we have a token; teardown on token change/clear.
  useEffect(() => {
    if (!token) return;
    const teardown = attachWS(qc);
    return teardown;
  }, [token, qc, attachWS]);

  // On mount, recover claims if token exists.
  useEffect(() => {
    if (!token) return;
    client
      .get("/auth/me", TokenClaims)
      .then(setClaims)
      .catch(() => {
        setToken(null);
        setStoredToken(null);
      });
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []); // mount only

  const login = useCallback(
    async (pin: string) => {
      const out = await client.post("/auth/login", LoginOutput, { pin });
      setToken(out.token);
      setStoredToken(out.token);
      setClaims(out.claims);
      setIsLocked(false);
    },
    [client],
  );

  const logout = useCallback(async () => {
    try {
      await client.post("/auth/logout", z.void());
    } catch {
      // ignore — tearing down regardless
    }
    setToken(null);
    setClaims(null);
    setStoredToken(null);
    setIsLocked(false);
  }, [client]);

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
