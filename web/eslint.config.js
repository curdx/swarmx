import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";

// ESLint 9+ flat config. Scoped narrow on purpose: the 53 `eslint-disable`
// comments scattered through src/ were ghosts (no ESLint ran), so this is the
// first real lint. We start with the two load-bearing React Hooks rules —
// `rules-of-hooks` (a genuine correctness gate) and `exhaustive-deps` (warn).
// The aggressive React-Compiler rules that ship in react-hooks v7's
// `recommended-latest` (purity / immutability / static-components / …) are very
// noisy on an existing tree; enable them incrementally once it's clean.
export default tseslint.config(
  {
    ignores: [
      "dist/**",
      "src-tauri/**",
      "node_modules/**",
      "**/*.config.{ts,js,mjs}",
    ],
  },
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      parser: tseslint.parser,
      parserOptions: { ecmaFeatures: { jsx: true } },
    },
    // Register @typescript-eslint too (without enabling its rules yet) so the
    // existing `eslint-disable @typescript-eslint/...` comments resolve to a
    // known rule instead of erroring with "rule definition not found".
    plugins: { "@typescript-eslint": tseslint.plugin, "react-hooks": reactHooks },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
    },
  },
);
