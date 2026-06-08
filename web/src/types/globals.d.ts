// Compile-time constants injected by vite.config.ts define block.
declare const __APP_VERSION__: string;

// vite 注入的 import.meta.env。main.tsx 用 PROD gate 生产环境的右键屏蔽。
interface ImportMetaEnv {
  readonly PROD: boolean;
  readonly DEV: boolean;
  readonly MODE: string;
  readonly VITE_ENABLE_DEBUG?: string;
}
interface ImportMeta {
  readonly env: ImportMetaEnv;
}
