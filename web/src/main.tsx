import ReactDOM from "react-dom/client";
import App from "./App";
import "@xterm/xterm/css/xterm.css";
import "./styles/global.css";

// StrictMode intentionally disabled: PTY + WebSocket per agent are
// single-consumer in M1 (server takes output_rx on attach, does not return it
// on detach), so StrictMode's mount-unmount-remount cycle leaves the second
// mount stranded with "agent already attached". M2's multi-attach (subscribe
// via broadcast) will let us re-enable StrictMode.
ReactDOM.createRoot(document.getElementById("root")!).render(<App />);
