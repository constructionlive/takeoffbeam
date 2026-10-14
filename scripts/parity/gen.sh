#!/usr/bin/env bash
# Regenerates crates/takeoff-core/tests/fixtures/platform_parity.json from the platform's TS.
set -euo pipefail
: "${PLATFORM:?set PLATFORM to a construction.live checkout}"
HERE="$(cd "$(dirname "$0")" && pwd)"
OUT="$HERE/../../crates/takeoff-core/tests/fixtures/platform_parity.json"
TMP="$(mktemp -d)"
"$PLATFORM/node_modules/.bin/esbuild" "$HERE/gen.ts" --bundle --platform=node --format=cjs \
	--alias:@platform="$PLATFORM" --outfile="$TMP/gen.cjs" --log-level=warning
node "$TMP/gen.cjs" > "$OUT"
rm -rf "$TMP"
echo "wrote $OUT"
