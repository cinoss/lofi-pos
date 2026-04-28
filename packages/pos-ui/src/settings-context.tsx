import { createContext, useContext } from "react";
import type { ReactNode } from "react";
import { useQuery } from "@tanstack/react-query";
import type { ApiClient } from "@lofi-pos/shared";
import { Settings } from "@lofi-pos/shared";
import type { Settings as SettingsType } from "@lofi-pos/shared";
import { useAuth } from "./auth-context";

const SettingsContext = createContext<SettingsType | null>(null);

export function SettingsProvider({
  client,
  children,
}: {
  client: ApiClient;
  children: ReactNode;
}) {
  const { isAuthenticated } = useAuth();
  const { data } = useQuery({
    queryKey: ["settings"],
    queryFn: () => client.get("/settings", Settings),
    enabled: isAuthenticated,
    staleTime: 5 * 60 * 1000,
  });
  return (
    <SettingsContext.Provider value={data ?? null}>
      {children}
    </SettingsContext.Provider>
  );
}

export function useSettings(): SettingsType | null {
  return useContext(SettingsContext);
}
