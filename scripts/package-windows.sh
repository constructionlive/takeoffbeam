#!/usr/bin/env bash
# Builds dist/Takeoff-Desk-<version>-x64-setup.exe (NSIS) — cross-compiled from
# macOS/Linux with mingw-w64 (`brew install mingw-w64 makensis`), or run in CI.
# Bundles pdfium.dll and llama.cpp's Vulkan build (NVIDIA/AMD/Intel GPUs, CPU fallback).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
TARGET="${TARGET:-x86_64-pc-windows-gnu}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target-win}"
STAGE="$ROOT/dist/windows/stage"
VENDOR="$ROOT/vendor/windows"

echo "› building $TARGET"
cargo build --release --target "$TARGET" -p takeoff-app -p takeoff-cli
BIN="$CARGO_TARGET_DIR/$TARGET/release"

mkdir -p "$VENDOR"
if [ ! -f "$VENDOR/pdfium/bin/pdfium.dll" ]; then
	echo "› fetching pdfium (win-x64, chromium/7881)"
	mkdir -p "$VENDOR/pdfium"
	curl -sSLf "https://github.com/bblanchon/pdfium-binaries/releases/download/chromium%2F7881/pdfium-win-x64.tgz" | tar xz -C "$VENDOR/pdfium"
fi
if [ ! -f "$VENDOR/llama/llama-server.exe" ]; then
	echo "› fetching llama.cpp (win-vulkan-x64)"
	URL="$(curl -sSf "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20" | LLAMA_TAG="${LLAMA_TAG:-}" python3 -c "
import json,os,sys
pin=os.environ.get('LLAMA_TAG')
for r in json.load(sys.stdin):
    if pin and r['tag_name']!=pin: continue
    a=[x for x in r['assets'] if x['name'].endswith('bin-win-vulkan-x64.zip')]
    if a: print(a[0]['browser_download_url']); break")"
	[ -n "$URL" ] || { echo "no llama.cpp Windows build found" >&2; exit 1; }
	TMP="$(mktemp -d)"
	curl -sSLf -o "$TMP/l.zip" "$URL"
	mkdir -p "$VENDOR/llama" && unzip -q -o "$TMP/l.zip" -d "$TMP/x"
	SRV="$(find "$TMP/x" -iname llama-server.exe | head -1)"
	cp "$SRV" "$VENDOR/llama/"
	cp "$(dirname "$SRV")"/*.dll "$VENDOR/llama/"
	basename "$URL" > "$VENDOR/llama/VERSION"
	rm -rf "$TMP"
fi

if [ ! -f "$VENDOR/vc_redist.x64.exe" ]; then
	echo "› fetching the Visual C++ runtime installer (llama.cpp needs it)"
	curl -sSLf -o "$VENDOR/vc_redist.x64.exe" https://aka.ms/vs/17/release/vc_redist.x64.exe
fi

echo "› staging"
rm -rf "$STAGE" && mkdir -p "$STAGE/bin"
cp "$BIN/takeoff-desk.exe" "$BIN/takeoff.exe" "$STAGE/"
cp "$VENDOR/pdfium/bin/pdfium.dll" "$STAGE/"
cp "$VENDOR/llama/"* "$STAGE/bin/"
cp LICENSE "$STAGE/LICENSE.txt"
cp NOTICE "$STAGE/NOTICE.txt"

# Native Windows tools (makensis) need C:\ paths, not Git-Bash /c/ paths.
winpath() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else echo "$1"; fi; }

echo "› building installer"
OUT="$ROOT/dist/Takeoff-Desk-${VERSION}-x64-setup.exe"
"${MAKENSIS:-makensis}" -V2 -DVERSION="$VERSION" -DSTAGE="$(winpath "$STAGE")" -DOUTFILE="$(winpath "$OUT")" -DVCREDIST="$(winpath "$VENDOR/vc_redist.x64.exe")" "$(winpath "$ROOT/installer/windows/takeoff-desk.nsi")"
echo "✓ $OUT ($(du -h "$OUT" | cut -f1))"
