import { defineConfig } from "vitest/config";
import path from "node:path";

// Unit tests live next to the code under src/. The Playwright e2e specs under
// tests/e2e/ are NOT vitest's — they use `@playwright/test`'s own `test()` and
// must be excluded here, or vitest tries to run them and errors.
export default defineConfig({
  // Mirror vite.config.ts's `@` → src alias so tests can import modules that
  // use the `@/…` path (e.g. lib/agent.ts → `@/i18n`). Without it vitest can't
  // resolve `@/` and any test that transitively pulls such a module fails to
  // load.
  resolve: {
    alias: {
      "@": path.resolve(__dirname, "./src"),
    },
  },
  test: {
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
  },
});
