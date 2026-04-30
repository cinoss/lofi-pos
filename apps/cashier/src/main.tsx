import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import { I18nProvider } from "@lingui/react";
import {
  ApiClientProvider,
  AuthProvider,
  SettingsProvider,
  attachWS as attachWSFactory,
} from "@lofi-pos/pos-ui";
import "./index.css";
import App from "./App";
import { apiClient, WS_BASE, getStoredToken } from "./lib/api";
import { i18n, setLocale, normalizeLocale } from "./lib/i18n";

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
