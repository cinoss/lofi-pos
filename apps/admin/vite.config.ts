import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";

// Mounted at /ui/admin/* by the cashier's axum (see http/static_admin.rs).
// `base` makes asset URLs absolute under that prefix; `BrowserRouter`
// is configured with the same `basename` in main.tsx.
export default defineConfig({
  base: "/ui/admin/",
  plugins: [react({ babel: { plugins: ["macros"] } }), tailwindcss()],
  clearScreen: false,
  server: {
    port: 1421,
    strictPort: true,
    // HMR ws connects DIRECTLY to vite even though the page is loaded
    // through the cashier's axum proxy at :7878/ui/admin/. Without this,
    // the client tries ws://localhost:7878/ which our proxy doesn't upgrade.
    hmr: { clientPort: 1421, host: "localhost", protocol: "ws" },
  },
});
