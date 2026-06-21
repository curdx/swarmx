#!/usr/bin/env bash
# Build the three binaries Tauri ships as sidecars and stage them under
# binaries/<name>-<target-triple> so `tauri build` can pick them up via
# the externalBin manifest entries.
#
# Why three: swarmx-server is the HTTP/WS entry point, but it spawns
# swarmx-shim for every agent (which in turn loads swarmx-mcp via the
# MCP config). All three resolve siblings off `current_exe()` parent, so
# Tauri must drop them in the same Contents/MacOS dir or swarm runs die
# with "swarmx-shim not found".
#
# Usage:
#   ./scripts/build-sidecar.sh                    # debug profile (fast iteration)
#   ./scripts/build-sidecar.sh release            # release profile for the host triple
#   ./scripts/build-sidecar.sh release <triple>   # release profile for an explicit target

set -euo pipefail

PROFILE="${1:-debug}"
REQUESTED_TARGET="${2:-}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
HOST_TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"
TARGET="${REQUESTED_TARGET:-$HOST_TARGET}"

if [[ -z "$HOST_TARGET" ]]; then
  echo "error: could not determine host target triple from rustc -vV" >&2
  exit 1
fi
if [[ -z "$TARGET" ]]; then
  echo "error: target triple is empty" >&2
  exit 1
fi

mkdir -p "$HERE/binaries"

CRATES=(swarmx-server swarmx-shim swarmx-mcp)

CARGO_ARGS=(build)
for c in "${CRATES[@]}"; do
  CARGO_ARGS+=(-p "$c")
done
CARGO_ARGS+=(--target "$TARGET")
if [[ "$PROFILE" == "release" ]]; then
  CARGO_ARGS+=(--release)
  TARGET_DIR="$REPO_ROOT/target/$TARGET/release"
else
  TARGET_DIR="$REPO_ROOT/target/$TARGET/debug"
fi

echo "→ cargo ${CARGO_ARGS[*]}  (in $REPO_ROOT)"
( cd "$REPO_ROOT" && cargo "${CARGO_ARGS[@]}" )

BIN_EXT=""
if [[ "$TARGET" == *"windows"* ]]; then
  BIN_EXT=".exe"
fi

for c in "${CRATES[@]}"; do
  SRC="$TARGET_DIR/$c$BIN_EXT"
  OUT="$HERE/binaries/$c-$TARGET$BIN_EXT"
  echo "→ cp $SRC → $OUT"
  cp "$SRC" "$OUT"
  chmod +x "$OUT"
done

echo "✓ sidecars ready in $HERE/binaries/"
