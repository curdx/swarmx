import ReactDOM from "react-dom/client";
import App from "./App";
import { applyInitialTheme, setTheme, getThemeMode } from "./lib/theme";
import "@xterm/xterm/css/xterm.css";
import "./styles/global.css";

// Apply the persisted theme BEFORE React renders so the first paint is
// already correct (no light → dark flash for users on dark mode).
applyInitialTheme();
// Wire the system-pref listener if the saved mode is "system".
setTheme(getThemeMode());

// StrictMode intentionally disabled: PTY + WebSocket per agent are
// single-consumer in M1 (server takes output_rx on attach, does not return it
// on detach), so StrictMode's mount-unmount-remount cycle leaves the second
// mount stranded with "agent already attached". M2's multi-attach (subscribe
// via broadcast) will let us re-enable StrictMode.
ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
