#!/usr/bin/env bash
# Builds dist/Takeoff-Desk-<version>-x64-setup.exe (NSIS) — cross-compiled from
# macOS/Linux with mingw-w64 (`brew install mingw-w64 makensis`), or run in CI.
# Bundles pdfium.dll and llama.cpp's Vulkan build (NVIDIA/AMD/Intel GPUs, CPU fallback).
# WIN_SIGN, if set, is a command that Authenticode-signs the file named after it, e.g.
#   WIN_SIGN='jsign --storetype TRUSTEDSIGNING --keystore … --storepass … --alias …'
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

if [ ! -f "$VENDOR/crt/vcruntime140.dll" ]; then
	# llama.cpp needs the Visual C++ runtime. Its DLLs ship next to the programs that use them
	# (Microsoft allows this), so installing for one user needs no admin rights. They are taken
	# from Microsoft's redistributable: cabinets inside the exe, and a cabinet inside those.
	[ -f "$VENDOR/vc_redist.x64.exe" ] || curl -sSLf -o "$VENDOR/vc_redist.x64.exe" https://aka.ms/vs/17/release/vc_redist.x64.exe
	echo "› extracting the Visual C++ runtime"
	TAR=tar; [ -x /c/Windows/System32/tar.exe ] && TAR=/c/Windows/System32/tar.exe # bsdtar reads cabinets
	TMP="$(mktemp -d)"
	python3 -I - "$VENDOR/vc_redist.x64.exe" "$TMP" "$TAR" <<'PY'
import os, struct, subprocess, sys
exe, tmp, tar = sys.argv[1:]
def cabs(data):
	i = 0
	while (i := data.find(b"MSCF\0\0\0\0", i)) >= 0:
		size = struct.unpack("<I", data[i + 8:i + 12])[0]
		if 0 < size <= len(data) - i:
			yield data[i:i + size]
			i += size
		else:
			i += 4
def unpack(data, name):
	d = os.path.join(tmp, name)
	os.makedirs(d)
	open(d + ".cab", "wb").write(data)
	subprocess.run([tar, "-xf", d + ".cab", "-C", d], check=True)
	return d
for n, outer in enumerate(cabs(open(exe, "rb").read())):
	d = unpack(outer, f"o{n}")
	for f in sorted(os.listdir(d)):
		inner = open(os.path.join(d, f), "rb").read()
		if inner.startswith(b"MSCF") and b"vcruntime140.dll_amd64" in inner:
			x = unpack(inner, f"i{n}{f}")
			os.makedirs(os.path.join(tmp, "crt"))
			for g in os.listdir(x):
				if g.endswith("_amd64") and g.startswith(("vcruntime140", "msvcp140")):
					os.rename(os.path.join(x, g), os.path.join(tmp, "crt", g[:-len("_amd64")]))
			sys.exit(0)
sys.exit("no x64 runtime found in vc_redist.x64.exe")
PY
	mkdir -p "$VENDOR/crt" && cp "$TMP/crt/"*.dll "$VENDOR/crt/"
	rm -rf "$TMP"
fi

echo "› staging"
rm -rf "$STAGE" && mkdir -p "$STAGE/bin"
cp "$BIN/takeoff-desk.exe" "$BIN/takeoff.exe" "$STAGE/"
cp "$VENDOR/pdfium/bin/pdfium.dll" "$STAGE/"
cp "$VENDOR/llama/"* "$STAGE/bin/"
cp "$VENDOR/crt/"*.dll "$STAGE/bin/"
cp "$VENDOR/crt/"*.dll "$STAGE/" # an MSVC build of the app needs them too
cp LICENSE "$STAGE/LICENSE.txt"
cp NOTICE "$STAGE/NOTICE.txt"

# Native Windows tools (makensis) need C:\ paths, not Git-Bash /c/ paths.
winpath() { if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else echo "$1"; fi; }

SIGN_ARGS=()
if [ -n "${WIN_SIGN:-}" ]; then
	echo "› signing"
	# Everything we ship except Microsoft's own runtime DLLs, which Microsoft has signed.
	find "$STAGE" -type f \( -iname '*.exe' -o -iname '*.dll' \) ! -iname 'vcruntime140*' ! -iname 'msvcp140*' |
		while read -r f; do eval "$WIN_SIGN \"\$f\""; done
	# makensis signs the uninstaller and the installer through this, after it writes each one.
	SIGN_SH="$ROOT/scripts/sign-windows.sh"
	command -v cygpath >/dev/null 2>&1 && SIGN_SH="$(cygpath -m "$SIGN_SH")"
	SIGN_ARGS=(-DSIGN="bash \"$SIGN_SH\"")
fi

echo "› building installer"
OUT="$ROOT/dist/Takeoff-Desk-${VERSION}-x64-setup.exe"
"${MAKENSIS:-makensis}" -V2 -DVERSION="$VERSION" -DSTAGE="$(winpath "$STAGE")" -DOUTFILE="$(winpath "$OUT")" ${SIGN_ARGS[@]+"${SIGN_ARGS[@]}"} "$(winpath "$ROOT/installer/windows/takeoff-desk.nsi")"
echo "✓ $OUT ($(du -h "$OUT" | cut -f1))"
