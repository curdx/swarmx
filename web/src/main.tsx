import ReactDOM from "react-dom/client";
import App from "./App";
import { applyInitialTheme, setTheme, getThemeMode } from "./lib/theme";
import "./i18n"; // side-effect init i18next
import "@xterm/xterm/css/xterm.css";
import "./styles/global.css";

// Apply the persisted theme BEFORE React renders so the first paint is
// already correct (no light → dark flash for users on dark mode).
applyInitialTheme();
// Wire the system-pref listener if the saved mode is "system".
setTheme(getThemeMode());

// 禁掉浏览器原生右键菜单 — 桌面 app 不该让用户看到 "翻译 / 检查 / 搜索"
// 这种网页味很重的选项 (Linear / Slack / Cursor 同款行为)。规则跟着 CSS
// `user-select` 走 — 能选 = 能右键复制，能选 ≠ 拦右键。这样 selection
// 和 contextmenu 体验对齐，没"能拖选但右键不弹"的尴尬：
//
//   1. 用户已经选中了文字 → 放行 (兜底；拖选完任何文字都能 "Copy")
//   2. 右键目标显式 user-select: text → 放行 (输入框 / .selectable /
//      pre / code / .xterm / .prose-context 这类内容容器)
//   3. 其他 (chrome 按钮 / nav / tab、未标记的 div span、空白) → 拦
//
// `auto` 不放行是为了避免"空白区域右键弹菜单"——空白 div 默认是 auto，
// 用户在空白右键期望没反应，跟 macOS Finder 同。要在普通文字上右键复
// 制，先拖选再右键即可（走兜底分支 1）。
//
// `import.meta.env.PROD` gate：dev build 整段 tree-shake，方便开发 Inspect。
if (import.meta.env.PROD) {
  document.addEventListener("contextmenu", (e) => {
    const sel = window.getSelection();
    if (sel && sel.toString().length > 0) return;
    const target = e.target as Element | null;
    if (!target) return;
    const cs = window.getComputedStyle(target);
    const us = cs.webkitUserSelect || cs.userSelect;
    if (us === "text") return;
    e.preventDefault();
  });
}

// StrictMode intentionally disabled: PTY + WebSocket per agent are
// single-consumer in M1 (server takes output_rx on attach, does not return it
// on detach), so StrictMode's mount-unmount-remount cycle leaves the second
// mount stranded with "agent already attached". M2's multi-attach (subscribe
// via broadcast) will let us re-enable StrictMode.
ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
