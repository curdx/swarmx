import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";

// During dev, the frontend runs on :5173 and the Rust server on :7777.
// Both /api and /ws are proxied so the same code paths work in production
// (where the server statically hosts the built bundle on :7777).
export default defineConfig({
  plugins: [react()],
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
