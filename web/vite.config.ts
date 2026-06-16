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

// Backend host:port the dev/preview proxy points at. Defaults to the standard
// dev server on :7777; override with FLOCKMUX_BACKEND (e.g. "127.0.0.1:7788")
// to run an isolated test stack against a separate backend without disturbing
// a long-lived :7777 dev session.
const BACKEND = process.env.FLOCKMUX_BACKEND || "127.0.0.1:7777";

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
      "/api": `http://${BACKEND}`,
      "/ws": {
        target: `ws://${BACKEND}`,
        ws: true,
        changeOrigin: true,
      },
    },
  },
  // `vite preview` proxy — used by the M.5 test recipe to run the built
  // dist/ against a parallel sidecar instance on :7778, so we don't have
  // to kill the long-lived dev backend on :7777.
  preview: {
    port: 4173,
    proxy: {
      "/api": `http://${BACKEND}`,
      "/ws": {
        target: `ws://${BACKEND}`,
        ws: true,
        changeOrigin: true,
      },
    },
  },
  build: {
    // Main route chunks are kept under the default 500KB warning threshold.
    // Mermaid's optional diagram internals still emit a couple of lazy chunks
    // around 560-615KB; they load only when an agent message previews a diagram.
    chunkSizeWarningLimit: 650,
    rollupOptions: {
      output: {
        manualChunks(id) {
          if (!id.includes("node_modules")) return undefined;
          if (
            id.includes("/react/") ||
            id.includes("/react-dom/") ||
            id.includes("/react-router-dom/") ||
            id.includes("/scheduler/") ||
            id.includes("/radix-ui/") ||
            id.includes("/@radix-ui/") ||
            id.includes("/cmdk/")
          ) {
            return "vendor-react-ui";
          }
          if (id.includes("/lucide-react/")) return "vendor-icons";
          if (
            id.includes("/react-markdown/") ||
            id.includes("/remark-") ||
            id.includes("/rehype-") ||
            id.includes("/highlight.js/") ||
            id.includes("/hast-") ||
            id.includes("/mdast-") ||
            id.includes("/micromark") ||
            id.includes("/unified/") ||
            id.includes("/unist-")
          ) {
            return "vendor-markdown";
          }
          if (id.includes("/recharts/")) return "vendor-charts";
          if (id.includes("/@xterm/")) return "vendor-xterm";
          return undefined;
        },
      },
    },
  },
});
