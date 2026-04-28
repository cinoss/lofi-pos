import { createContext, useContext } from "react";
import type { ReactNode } from "react";
import type { ApiClient } from "@lofi-pos/shared";

const ApiClientContext = createContext<ApiClient | null>(null);

export function ApiClientProvider({
  client,
  children,
}: {
  client: ApiClient;
  children: ReactNode;
}) {
  return (
    <ApiClientContext.Provider value={client}>
      {children}
    </ApiClientContext.Provider>
  );
}

export function useApiClient(): ApiClient {
  const c = useContext(ApiClientContext);
  if (!c) throw new Error("useApiClient outside ApiClientProvider");
  return c;
}
