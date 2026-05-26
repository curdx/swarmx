#!/usr/bin/env bash
# Build the three binaries Tauri ships as sidecars and stage them under
# binaries/<name>-<target-triple> so `tauri build` can pick them up via
# the externalBin manifest entries.
#
# Why three: flockmux-server is the HTTP/WS entry point, but it spawns
# flockmux-shim for every agent (which in turn loads flockmux-mcp via the
# MCP config). All three resolve siblings off `current_exe()` parent, so
# Tauri must drop them in the same Contents/MacOS dir or swarm runs die
# with "flockmux-shim not found".
#
# Usage:
#   ./scripts/build-sidecar.sh           # debug profile (fast iteration)
#   ./scripts/build-sidecar.sh release   # release profile (what tauri build needs)

set -euo pipefail

PROFILE="${1:-debug}"
HERE="$(cd "$(dirname "$0")/.." && pwd)"
REPO_ROOT="$(cd "$HERE/../.." && pwd)"
TARGET="$(rustc -vV | awk '/^host:/ {print $2}')"

if [[ -z "$TARGET" ]]; then
  echo "error: could not determine host target triple from rustc -vV" >&2
  exit 1
fi

mkdir -p "$HERE/binaries"

CRATES=(flockmux-server flockmux-shim flockmux-mcp)

CARGO_ARGS=(build)
for c in "${CRATES[@]}"; do
  CARGO_ARGS+=(-p "$c")
done
if [[ "$PROFILE" == "release" ]]; then
  CARGO_ARGS+=(--release)
  TARGET_DIR="$REPO_ROOT/target/release"
else
  TARGET_DIR="$REPO_ROOT/target/debug"
fi

echo "→ cargo ${CARGO_ARGS[*]}  (in $REPO_ROOT)"
( cd "$REPO_ROOT" && cargo "${CARGO_ARGS[@]}" )

for c in "${CRATES[@]}"; do
  SRC="$TARGET_DIR/$c"
  OUT="$HERE/binaries/$c-$TARGET"
  echo "→ cp $SRC → $OUT"
  cp "$SRC" "$OUT"
  chmod +x "$OUT"
done

echo "✓ sidecars ready in $HERE/binaries/"
