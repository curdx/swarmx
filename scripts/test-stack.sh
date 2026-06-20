#!/usr/bin/env bash
#
# Isolated full dev stack for manual / agent UI testing.
#
# Runs the Rust backend AND a Vite dev frontend on NON-default ports (7788 /
# 5188) with a throwaway /tmp data dir, so it never collides with a long-lived
# `:7777` + `:5173` dev session you're already running. The frontend is real
# Vite dev (hot-reload), proxying /api + /ws to the isolated backend via the
# `FLOCKMUX_BACKEND` override (see web/vite.config.ts).
#
# Why this exists: API/curl tests miss UI-only failures (a backend capability
# with no UI entry point; a flow that only breaks through the rendered app). The
# verify-via-real-ui rule says drive the actual UI. This boots a clean stack to
# drive (with chrome-devtools MCP) without touching your working session.
#
#   bash scripts/test-stack.sh         # build + start; leaves it running
#   bash scripts/test-stack.sh stop    # tear it down
#
# Open http://localhost:5188 once it's up.
set -uo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BPORT="${FLOCKMUX_TEST_BACKEND_PORT:-7788}"
FPORT="${FLOCKMUX_TEST_FRONTEND_PORT:-5188}"
DATA="${FLOCKMUX_TEST_DATA:-/tmp/flockmux-teststack}"
PIDFILE="$DATA/pids"

stop() {
  if [ -f "$PIDFILE" ]; then
    while read -r p; do [ -n "$p" ] && kill "$p" 2>/dev/null; done < "$PIDFILE"
    rm -f "$PIDFILE"
  fi
  # Belt-and-suspenders: anything still bound to our ports (child CLIs die via
  # kill_on_drop when the backend goes).
  lsof -ti :"$BPORT" 2>/dev/null | xargs kill 2>/dev/null || true
  lsof -ti :"$FPORT" 2>/dev/null | xargs kill 2>/dev/null || true
  echo "✓ test stack stopped (:$BPORT / :$FPORT)"
}

if [ "${1:-}" = "stop" ]; then stop; exit 0; fi

stop  # clean any prior instance on these ports first
mkdir -p "$DATA"/{ws,bb,rec}
cd "$ROOT"

echo "▶ building backend (cargo)…"
cargo build -p flockmux-server -q || { echo "✗ cargo build failed"; exit 1; }

echo "▶ starting backend on :$BPORT (isolated data: $DATA)…"
nohup env FLOCKMUX_PORT="$BPORT" FLOCKMUX_SERVER_URL="http://127.0.0.1:$BPORT" \
  FLOCKMUX_DB_PATH="$DATA/d.db" FLOCKMUX_WORKSPACES_DIR="$DATA/ws" \
  FLOCKMUX_BLACKBOARD_DIR="$DATA/bb" FLOCKMUX_RECORDINGS_DIR="$DATA/rec" \
  RUST_LOG="${RUST_LOG:-warn,flockmux_server=info}" \
  "$ROOT/target/debug/flockmux-server" > "$DATA/backend.log" 2>&1 &
echo $! > "$PIDFILE"

echo "▶ starting Vite dev on :$FPORT (proxy → :$BPORT)…"
cd "$ROOT/web"
[ -d node_modules ] || npm_config_cache=/tmp/.npm-flockmux npm install
nohup env FLOCKMUX_BACKEND="127.0.0.1:$BPORT" \
  npx vite --port "$FPORT" --strictPort > "$DATA/frontend.log" 2>&1 &
echo $! >> "$PIDFILE"

for _ in $(seq 1 60); do curl -sf "http://127.0.0.1:$BPORT/api/plugins" >/dev/null 2>&1 && break; sleep 1; done
for _ in $(seq 1 60); do curl -sf "http://localhost:$FPORT/" >/dev/null 2>&1 && break; sleep 1; done

curl -sf "http://127.0.0.1:$BPORT/api/plugins" >/dev/null 2>&1 \
  && echo "  backend  ✓ http://127.0.0.1:$BPORT" \
  || { echo "  backend  ✗ did not come up; tail:"; tail -8 "$DATA/backend.log"; }
curl -sf "http://localhost:$FPORT/" >/dev/null 2>&1 \
  && echo "  frontend ✓ http://localhost:$FPORT" \
  || { echo "  frontend ✗ did not come up; tail:"; tail -8 "$DATA/frontend.log"; }

echo "────────────────────────────────"
echo "Open  →  http://localhost:$FPORT"
echo "Logs  →  $DATA/{backend,frontend}.log"
echo "Stop  →  bash scripts/test-stack.sh stop"
