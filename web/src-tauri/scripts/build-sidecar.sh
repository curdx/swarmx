#!/usr/bin/env bash
# Build flockmux-server and stage it as the Tauri sidecar binary.
#
# Tauri's externalBin manifest entry is `binaries/flockmux-server`; at bundle
# time the CLI looks for `binaries/flockmux-server-${TARGET_TRIPLE}` and
# embeds it into the resulting .app / .exe / .AppImage.
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
OUT="$HERE/binaries/flockmux-server-$TARGET"

CARGO_ARGS=(build -p flockmux-server)
if [[ "$PROFILE" == "release" ]]; then
  CARGO_ARGS+=(--release)
  SRC="$REPO_ROOT/target/release/flockmux-server"
else
  SRC="$REPO_ROOT/target/debug/flockmux-server"
fi

echo "→ cargo ${CARGO_ARGS[*]}  (in $REPO_ROOT)"
( cd "$REPO_ROOT" && cargo "${CARGO_ARGS[@]}" )

echo "→ cp $SRC → $OUT"
cp "$SRC" "$OUT"
chmod +x "$OUT"

echo "✓ sidecar ready: $OUT"
