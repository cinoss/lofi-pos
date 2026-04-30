import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { I18nProvider } from "@lingui/react";
import { ApiClient } from "@lofi-pos/shared";
import {
  ApiClientProvider,
  AuthProvider,
  SettingsProvider,
  getStoredToken,
  attachWS as attachWSFactory,
} from "@lofi-pos/pos-ui";
import "./index.css";
import App from "./App";
import { i18n, setLocale, normalizeLocale } from "./lib/i18n";

const API_BASE =
  import.meta.env.VITE_API_BASE ?? window.location.origin;
const WS_BASE = API_BASE.replace(/^http/, "ws");

const apiClient = new ApiClient({ baseUrl: API_BASE, getToken: getStoredToken });
const queryClient = new QueryClient({
  defaultOptions: { queries: { staleTime: 1000, refetchOnWindowFocus: false } },
});

const attachWS = (qc: QueryClient) =>
  attachWSFactory({ baseUrl: WS_BASE, getToken: getStoredToken }, qc);

ReactDOM.createRoot(document.getElementById("root")!).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <I18nProvider i18n={i18n}>
        <ApiClientProvider client={apiClient}>
          <AuthProvider client={apiClient} attachWS={attachWS}>
            <SettingsProvider
              client={apiClient}
              onLoaded={(s) => setLocale(normalizeLocale(s.locale))}
            >
              <BrowserRouter>
                <App />
              </BrowserRouter>
            </SettingsProvider>
          </AuthProvider>
        </ApiClientProvider>
      </I18nProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
