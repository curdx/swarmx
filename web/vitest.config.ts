import { defineConfig } from "vitest/config";

// Unit tests live next to the code under src/. The Playwright e2e specs under
// tests/e2e/ are NOT vitest's — they use `@playwright/test`'s own `test()` and
// must be excluded here, or vitest tries to run them and errors.
export default defineConfig({
  test: {
    include: ["src/**/*.{test,spec}.{ts,tsx}"],
  },
});
