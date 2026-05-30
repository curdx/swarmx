#!/usr/bin/env bash
#
# Golden per-CLI acceptance test — L3 of docs/multi-cli-redesign-plan.md.
#
# For each cli-plugin, headless-spawn a worker whose entire job is to call the
# `swarm_write_blackboard` MCP tool, then assert the key shows up on the
# blackboard. A PASS proves the whole chain works end to end for that CLI:
#
#     spawn → pre-spawn patches (trust / MCP inject / Stop-hook) → bootstrap
#     prompt reached the model → swarm_* MCP tools are wired into the toolset
#     → the swarm round-trip back to flockmux-server succeeds.
#
# This is the "adding a CLI is a testable contract" gate (modeled on
# superpowers' tests/skill-triggering/run-test.sh). It is deliberately NOT a
# `cargo test` / CI test: it spawns REAL CLIs, needs them installed + logged
# in (API keys), is non-deterministic (an LLM turn), and spends a small number
# of tokens (one short turn per CLI). Run it by hand after adding/changing a
# cli-plugins/<id>.toml:
#
#     bash scripts/golden-cli-test.sh            # claude + codex (defaults)
#     bash scripts/golden-cli-test.sh gemini     # just gemini
#
# A CLI whose binary isn't on PATH is SKIPPED (not failed). Exit code is
# non-zero iff at least one tested CLI FAILED.
#
# NOTE: the harness sets FLOCKMUX_SERVER_URL to match FLOCKMUX_PORT. This is
# load-bearing — if they diverge, the agent's flockmux-mcp + wake-check call
# the default :7777 instead of this test server, and every CLI "fails".
set -uo pipefail

PORT="${FLOCKMUX_GOLDEN_PORT:-7798}"
BASE="http://127.0.0.1:${PORT}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CLIS=("$@")
[ ${#CLIS[@]} -eq 0 ] && CLIS=(claude codex)

echo "▶ building workspace binaries…"
cargo build --workspace -q || { echo "✗ cargo build failed"; exit 1; }

TMP="$(mktemp -d /tmp/flockmux-golden.XXXX)"
mkdir -p "$TMP/project"
printf '# golden cli test\n' > "$TMP/project/README.md"
SRV=""

cleanup() {
  # Reap any still-live agents (a failed worker won't have auto-killed) via the
  # full-teardown DELETE path so their process groups are reaped, then the
  # server, then the temp dir.
  if curl -sf "$BASE/api/plugins" >/dev/null 2>&1; then
    for a in $(curl -s "$BASE/api/agent" 2>/dev/null | python3 -c 'import sys,json
try:
    print("\n".join(x["agent_id"] for x in json.load(sys.stdin) if x.get("killed_at") is None))
except Exception:
    pass' 2>/dev/null); do
      curl -s -o /dev/null -X DELETE "$BASE/api/agent/$a" 2>/dev/null
    done
    sleep 1
  fi
  [ -n "$SRV" ] && kill "$SRV" 2>/dev/null
  rm -rf "$TMP" 2>/dev/null
}
trap cleanup EXIT

echo "▶ starting flockmux-server on :$PORT (isolated data dir)…"
FLOCKMUX_PORT="$PORT" FLOCKMUX_SERVER_URL="$BASE" \
  FLOCKMUX_DB_PATH="$TMP/d.db" FLOCKMUX_WORKSPACES_DIR="$TMP/ws" \
  FLOCKMUX_BLACKBOARD_DIR="$TMP/bb" FLOCKMUX_RECORDINGS_DIR="$TMP/rec" \
  RUST_LOG=warn ./target/debug/flockmux-server > "$TMP/server.log" 2>&1 &
SRV=$!

for _ in $(seq 1 30); do curl -sf "$BASE/api/plugins" >/dev/null 2>&1 && break; sleep 1; done
if ! curl -sf "$BASE/api/plugins" >/dev/null 2>&1; then
  echo "✗ server did not come up; log tail:"; tail -5 "$TMP/server.log"; exit 1
fi

WS="$(curl -s -X POST "$BASE/api/workspaces" -H 'Content-Type: application/json' \
  -d "{\"name\":\"golden\",\"cwd\":\"$TMP/project\"}" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')"

PASS=0; FAIL=0; SKIP=0
for cli in "${CLIS[@]}"; do
  bin="$(curl -s "$BASE/api/plugins" | CLI="$cli" python3 -c \
    'import os,sys,json;print(next((p["binary"] for p in json.load(sys.stdin) if p["id"]==os.environ["CLI"]),""))')"
  if [ -z "$bin" ]; then echo "• $cli: SKIP (no cli-plugins/$cli.toml)"; SKIP=$((SKIP+1)); continue; fi
  if ! command -v "$bin" >/dev/null 2>&1; then echo "• $cli: SKIP ($bin not on PATH)"; SKIP=$((SKIP+1)); continue; fi

  key="golden/${cli}.done"
  prompt="Connectivity self-test. Your ONLY task: immediately call the swarm_write_blackboard tool to write the exact text READY to the key ${key}. Do not create files, do not run commands, do not do anything else. After that single tool call you are finished."
  body="$(CLI="$cli" KEY="$key" WS="$WS" PROMPT="$prompt" python3 -c 'import os,json;print(json.dumps({
    "cli":os.environ["CLI"],"role_label":"golden","system_prompt":os.environ["PROMPT"],
    "handoff_signal":os.environ["KEY"],"caller_agent_id":"golden-harness","workspace_id":os.environ["WS"]}))')"

  echo "• $cli: spawning golden worker ($bin)…"
  curl -s -o /dev/null -X POST "$BASE/api/worker" -H 'Content-Type: application/json' -d "$body"

  ok=0
  for _ in $(seq 1 60); do
    if curl -s "$BASE/api/blackboard/$key" 2>/dev/null | grep -q 'READY'; then ok=1; break; fi
    sleep 2
  done
  if [ "$ok" = 1 ]; then
    echo "  ✓ $cli PASS — swarm_write_blackboard round-trip reached the model and the server"
    PASS=$((PASS+1))
  else
    echo "  ✗ $cli FAIL — key '$key' never appeared (bootstrap/MCP/tool wiring broken?)"
    FAIL=$((FAIL+1))
  fi
done

echo "────────────────────────────────"
echo "golden: ${PASS} passed · ${FAIL} failed · ${SKIP} skipped"
[ "$FAIL" -eq 0 ]
