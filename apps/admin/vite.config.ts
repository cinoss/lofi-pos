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
    // Forward backend API calls to the cashier so opening admin directly at
    // http://localhost:1421/ui/admin/ also works (not just via the cashier's
    // /ui/admin proxy at :7878). Anything that isn't the admin SPA itself
    // belongs to the cashier server.
    proxy: {
      "^/(admin|auth|sessions|spots|products|staff|settings|orders|reports)(/.*)?$":
        { target: "http://localhost:7878", changeOrigin: true },
      "/ws": {
        target: "ws://localhost:7878",
        ws: true,
        changeOrigin: true,
      },
    },
  },
});
