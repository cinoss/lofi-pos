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
  server: { port: 1421, strictPort: true },
});
