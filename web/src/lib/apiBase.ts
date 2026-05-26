// Single source of truth for backend URLs across runtimes.
//
//   dev (vite):     '/api' and '/ws' are proxied to :7777, so relative
//                   paths work and ws derives host from window.location.
//   prod web:       the built bundle is served by flockmux-server on
//                   :7777 itself → window.location.host *is* the backend,
//                   still relative.
//   prod tauri:     the webview's asset protocol (tauri://localhost or
//                   http(s)://tauri.localhost) doesn't route /api or /ws;
//                   point at 127.0.0.1:7777 directly. backend CORS layer
//                   is permissive so cross-origin fetch works.
//
// Detection is purely runtime: protocol === "tauri:" covers macOS asset
// scheme, hostname === "tauri.localhost" covers Windows/Linux. Tauri's dev
// build runs against vite's localhost so neither matches → proxy path
// stays in play, just like the browser.

const isTauriProd =
  typeof window !== "undefined" &&
  (window.location.protocol === "tauri:" ||
    window.location.hostname === "tauri.localhost");

export const HTTP_BASE = isTauriProd ? "http://127.0.0.1:7777" : "";
export const WS_HOST = isTauriProd ? "127.0.0.1:7777" : window.location.host;
export const WS_PROTO = isTauriProd
  ? "ws:"
  : window.location.protocol === "https:"
    ? "wss:"
    : "ws:";
