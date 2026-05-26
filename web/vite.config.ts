import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import tailwindcss from "@tailwindcss/vite";
import path from "node:path";
import { readFileSync } from "node:fs";

// During dev, the frontend runs on :5173 and the Rust server on :7777.
// Both /api and /ws are proxied so the same code paths work in production
// (where the server statically hosts the built bundle on :7777).

// Read version from package.json so About panel doesn't drift from the
// source of truth. JSON.parse + readFileSync is fine — vite.config runs
// once at startup, not per request.
const pkg = JSON.parse(
  readFileSync(path.resolve(__dirname, "./package.json"), "utf-8"),
) as { version: string };

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  define: {
    __APP_VERSION__: JSON.stringify(pkg.version),
  },
  server: {
    port: 5173,
    proxy: {
      "/api": "http://127.0.0.1:7777",
      "/ws": {
        target: "ws://127.0.0.1:7777",
        ws: true,
        changeOrigin: true,
      },
    },
  },
});
