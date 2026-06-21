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
#     → the swarm round-trip back to swarmx-server succeeds.
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
#     GOLDEN_MODEL_opencode=open-ai/gpt-5.5 \     # engines without a built-in
#       bash scripts/golden-cli-test.sh opencode  #   default model need one
#
# A CLI whose binary isn't on PATH is SKIPPED (not failed). Exit code is
# non-zero iff at least one tested CLI FAILED.
#
# NOTE: the harness sets SWARMX_SERVER_URL to match SWARMX_PORT. This is
# load-bearing — if they diverge, the agent's swarmx-mcp + wake-check call
# the default :7777 instead of this test server, and every CLI "fails".
set -uo pipefail

PORT="${SWARMX_GOLDEN_PORT:-7798}"
BASE="http://127.0.0.1:${PORT}"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

CLIS=("$@")
[ ${#CLIS[@]} -eq 0 ] && CLIS=(claude codex)

echo "▶ building workspace binaries…"
cargo build --workspace -q || { echo "✗ cargo build failed"; exit 1; }

TMP="$(mktemp -d /tmp/swarmx-golden.XXXX)"
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

echo "▶ starting swarmx-server on :$PORT (isolated data dir)…"
SWARMX_PORT="$PORT" SWARMX_SERVER_URL="$BASE" \
  SWARMX_DB_PATH="$TMP/d.db" SWARMX_WORKSPACES_DIR="$TMP/ws" \
  SWARMX_BLACKBOARD_DIR="$TMP/bb" SWARMX_RECORDINGS_DIR="$TMP/rec" \
  RUST_LOG=warn ./target/debug/swarmx-server > "$TMP/server.log" 2>&1 &
SRV=$!

for _ in $(seq 1 30); do curl -sf "$BASE/api/plugins" >/dev/null 2>&1 && break; sleep 1; done
if ! curl -sf "$BASE/api/plugins" >/dev/null 2>&1; then
  echo "✗ server did not come up; log tail:"; tail -5 "$TMP/server.log"; exit 1
fi

WS="$(curl -s -X POST "$BASE/api/workspaces" -H 'Content-Type: application/json' \
  -d "{\"name\":\"golden\",\"cwd\":\"$TMP/project\"}" \
  | python3 -c 'import sys,json;print(json.load(sys.stdin)["id"])')"

plugin_binary() { # $1=cli id → its manifest binary (empty if no such plugin)
  curl -s "$BASE/api/plugins" | CLI="$1" python3 -c \
    'import os,sys,json;print(next((p["binary"] for p in json.load(sys.stdin) if p["id"]==os.environ["CLI"]),""))'
}

# /api/worker authorizes the caller: caller_agent_id must already be a member
# of this workspace (anti cross-workspace escalation, rest.rs:1553). So we can't
# spawn the first worker from a bare harness id — we first spawn ONE lightweight
# caller agent via /api/agent (the first CLI under test that's installed) as the
# membership anchor. It sits idle and is reaped by cleanup.
CALLER=""
for c in "${CLIS[@]}"; do
  cbin="$(plugin_binary "$c")"
  [ -n "$cbin" ] && command -v "$cbin" >/dev/null 2>&1 || continue
  CALLER="$(curl -s -X POST "$BASE/api/agent" -H 'Content-Type: application/json' \
    -d "{\"cli\":\"$c\",\"role\":\"researcher\",\"workspace_id\":\"$WS\"}" \
    | python3 -c 'import sys,json
try: print(json.load(sys.stdin)["agent_id"])
except Exception: print("")')"
  [ -n "$CALLER" ] && { echo "▶ membership-anchor caller: $CALLER ($c)"; break; }
done
if [ -z "$CALLER" ]; then
  echo "✗ no installed CLI to anchor workspace membership; cannot spawn workers"; exit 1
fi
sleep 2

PASS=0; FAIL=0; SKIP=0
for cli in "${CLIS[@]}"; do
  bin="$(plugin_binary "$cli")"
  if [ -z "$bin" ]; then echo "• $cli: SKIP (no cli-plugins/$cli.toml)"; SKIP=$((SKIP+1)); continue; fi
  if ! command -v "$bin" >/dev/null 2>&1; then echo "• $cli: SKIP ($bin not on PATH)"; SKIP=$((SKIP+1)); continue; fi

  key="golden/${cli}.done"
  prompt="Connectivity self-test. Your ONLY task: immediately call the swarm_write_blackboard tool to write the exact text READY to the key ${key}. Do not create files, do not run commands, do not do anything else. After that single tool call you are finished."
  # Optional model override — some engines have no built-in default model (e.g.
  # opencode needs an explicit provider/model). SWARMX_GOLDEN_MODEL applies to
  # every CLI; GOLDEN_MODEL_<cli> overrides per CLI (e.g. GOLDEN_MODEL_opencode).
  mvar="GOLDEN_MODEL_${cli}"; model="${!mvar:-${SWARMX_GOLDEN_MODEL:-}}"
  body="$(CLI="$cli" KEY="$key" WS="$WS" CALLER="$CALLER" MODEL="$model" PROMPT="$prompt" python3 -c 'import os,json
b={"cli":os.environ["CLI"],"role":"researcher","system_prompt":os.environ["PROMPT"],
   "caller_agent_id":os.environ["CALLER"],"workspace_id":os.environ["WS"]}
if os.environ.get("MODEL"): b["model"]=os.environ["MODEL"]
print(json.dumps(b))')"

  echo "• $cli: spawning golden worker ($bin)…"
  resp="$(curl -s -X POST "$BASE/api/worker" -H 'Content-Type: application/json' -d "$body")"
  wid="$(echo "$resp" | python3 -c 'import sys,json
try: print(json.load(sys.stdin).get("agent_id",""))
except Exception: print("")' 2>/dev/null)"
  if [ -z "$wid" ]; then
    echo "  ✗ $cli FAIL — spawn rejected: $(echo "$resp" | head -c 200)"
    FAIL=$((FAIL+1)); continue
  fi

  ok=0
  # ~180s: one LLM turn, and some providers/proxies are slow or rate-limited.
  for _ in $(seq 1 90); do
    # Assert by HTTP status (200 = key exists), NOT a body grep: the key name
    # itself can contain words like "done", which made grep give false hits.
    if [ "$(curl -s -o /dev/null -w '%{http_code}' "$BASE/api/blackboard/$key")" = "200" ]; then ok=1; break; fi
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
