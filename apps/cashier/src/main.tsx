import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { AuthProvider, SettingsProvider, attachWS as attachWSFactory } from "@lofi-pos/pos-ui";
import "./index.css";
import App from "./App";
import { apiClient, WS_BASE, getStoredToken } from "./lib/api";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 1000, refetchOnWindowFocus: false },
  },
});

const attachWS = (qc: QueryClient) =>
  attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);

const root = document.getElementById("root")!;
ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider client={apiClient} attachWS={attachWS}>
        <SettingsProvider client={apiClient}>
          <BrowserRouter>
            <App />
          </BrowserRouter>
        </SettingsProvider>
      </AuthProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
