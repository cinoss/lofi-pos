import React from "react";
import ReactDOM from "react-dom/client";
import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { BrowserRouter } from "react-router-dom";
import App from "./App";
import { AuthProvider } from "./lib/auth-context";
import { attachWS } from "./lib/ws";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: { staleTime: 1000, refetchOnWindowFocus: false },
  },
});

// Attach WS once the app boots; StrictMode runs effects twice in dev so we
// gate at module scope (single attachment for the lifetime of the page).
let teardownWS: (() => void) | null = null;
if (!teardownWS) teardownWS = attachWS(queryClient);

const root = document.getElementById("root")!;
ReactDOM.createRoot(root).render(
  <React.StrictMode>
    <QueryClientProvider client={queryClient}>
      <AuthProvider>
        <BrowserRouter>
          <App />
        </BrowserRouter>
      </AuthProvider>
    </QueryClientProvider>
  </React.StrictMode>,
);
