#!/usr/bin/env bash
# Builds "Takeoff Desk.app" + a .dmg in dist/.
#   scripts/package-macos.sh              # unsigned (or ad-hoc) local build
#   SIGN_IDENTITY="Developer ID Application: …" scripts/package-macos.sh
#   NOTARIZE=1 APPLE_ID=… APPLE_TEAM_ID=… APPLE_APP_SPECIFIC_PASSWORD=… (with SIGN_IDENTITY)
# Bundles: the app, the `takeoff` CLI, libpdfium, and llama.cpp's llama-server
# (official ggml-org macOS release) so local models work out of the box.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
ARCH="$(uname -m)"
DIST="$ROOT/dist"
APP="$DIST/Takeoff Desk.app"
LLAMA_DIR="$ROOT/vendor/llama-macos-$ARCH"

[ -f vendor/pdfium/lib/libpdfium.dylib ] || scripts/fetch-pdfium.sh

echo "› building release binaries"
cargo build --release -p takeoff-app -p takeoff-cli

if [ ! -x "$LLAMA_DIR/llama-server" ]; then
	echo "› fetching llama.cpp (latest release, macOS $ARCH)"
	rm -rf "$LLAMA_DIR" && mkdir -p "$LLAMA_DIR"
	# Newest build release with a macOS asset (or LLAMA_TAG=b11617 to pin one).
	ASSET="$(curl -sSf "https://api.github.com/repos/ggml-org/llama.cpp/releases?per_page=20" | LLAMA_TAG="${LLAMA_TAG:-}" python3 -c "
import json,os,sys
arch='arm64' if '$ARCH'=='arm64' else 'x64'
pin=os.environ.get('LLAMA_TAG')
for r in json.load(sys.stdin):
    if pin and r['tag_name']!=pin: continue
    a=[x for x in r['assets'] if x['name'].endswith((f'bin-macos-{arch}.zip', f'bin-macos-{arch}.tar.gz'))]
    if a: print(a[0]['browser_download_url']); break")"
	[ -n "$ASSET" ] || { echo "no llama.cpp macOS build found" >&2; exit 1; }
	TMP="$(mktemp -d)"
	curl -sSLf -o "$TMP/llama.pkg" "$ASSET"
	case "$ASSET" in
		*.zip) unzip -q "$TMP/llama.pkg" -d "$TMP/x" ;;
		*) mkdir -p "$TMP/x" && tar xzf "$TMP/llama.pkg" -C "$TMP/x" ;;
	esac
	SRV="$(find "$TMP/x" -name llama-server -type f | head -1)"
	cp "$SRV" "$LLAMA_DIR/"
	cp "$(dirname "$SRV")"/*.dylib "$LLAMA_DIR/" 2>/dev/null || true
	find "$TMP/x" -name "*.metal" -exec cp {} "$LLAMA_DIR/" \; 2>/dev/null || true
	basename "$ASSET" > "$LLAMA_DIR/VERSION"
	rm -rf "$TMP"
fi

echo "› assembling $APP"
rm -rf "$APP" && mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Frameworks" "$APP/Contents/Resources/bin"
cp target/release/takeoff-desk "$APP/Contents/MacOS/"
cp target/release/takeoff "$APP/Contents/Resources/bin/"
cp vendor/pdfium/lib/libpdfium.dylib "$APP/Contents/Frameworks/"
cp -a "$LLAMA_DIR"/. "$APP/Contents/Resources/bin/"
cp NOTICE LICENSE "$APP/Contents/Resources/"

# Icon: SVG → iconset → icns.
ICONSET="$(mktemp -d)/TakeoffDesk.iconset"
mkdir -p "$ICONSET"
for s in 16 32 64 128 256 512 1024; do
	cargo run -q --release -p takeoff-pdf --example svg2png -- assets/icon.svg "$ICONSET/icon_${s}.png" "$s"
done
for s in 16 32 128 256 512; do
	cp "$ICONSET/icon_${s}.png" "$ICONSET/icon_${s}x${s}.png"
	d=$((s * 2)); cp "$ICONSET/icon_${d}.png" "$ICONSET/icon_${s}x${s}@2x.png"
done
rm -f "$ICONSET"/icon_{16,32,64,128,256,512,1024}.png
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/TakeoffDesk.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
	<key>CFBundleName</key><string>Takeoff Desk</string>
	<key>CFBundleDisplayName</key><string>Takeoff Desk</string>
	<key>CFBundleIdentifier</key><string>live.construction.takeoffdesk</string>
	<key>CFBundleVersion</key><string>${VERSION}</string>
	<key>CFBundleShortVersionString</key><string>${VERSION}</string>
	<key>CFBundleExecutable</key><string>takeoff-desk</string>
	<key>CFBundleIconFile</key><string>TakeoffDesk</string>
	<key>CFBundlePackageType</key><string>APPL</string>
	<key>LSMinimumSystemVersion</key><string>12.0</string>
	<key>NSHighResolutionCapable</key><true/>
	<key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
	<key>NSHumanReadableCopyright</key><string>© 2026 construction.live · Apache-2.0</string>
	<key>CFBundleDocumentTypes</key><array><dict>
		<key>CFBundleTypeName</key><string>PDF drawing</string>
		<key>CFBundleTypeRole</key><string>Editor</string>
		<key>LSHandlerRank</key><string>Alternate</string>
		<key>LSItemContentTypes</key><array><string>com.adobe.pdf</string></array>
	</dict></array>
</dict></plist>
PLIST

if [ -n "${SIGN_IDENTITY:-}" ]; then
	echo "› signing with $SIGN_IDENTITY"
	ENT="$(mktemp)"
	cat > "$ENT" <<ENTXML
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
	<key>com.apple.security.cs.disable-library-validation</key><true/>
</dict></plist>
ENTXML
	# Inside-out: every Mach-O first, then the bundle.
	while IFS= read -r f; do
		if file -b "$f" | grep -q "Mach-O"; then
			codesign --force --timestamp --options runtime --entitlements "$ENT" --sign "$SIGN_IDENTITY" "$f"
		fi
	done < <(find "$APP/Contents/Frameworks" "$APP/Contents/Resources/bin" -type f)
	codesign --force --timestamp --options runtime --entitlements "$ENT" --sign "$SIGN_IDENTITY" "$APP/Contents/MacOS/takeoff-desk"
	codesign --force --timestamp --options runtime --entitlements "$ENT" --sign "$SIGN_IDENTITY" "$APP"
	codesign --verify --deep --strict "$APP"
else
	echo "› ad-hoc signing (set SIGN_IDENTITY for a distributable build)"
	codesign --force --deep --sign - "$APP"
fi

echo "› building dmg"
DMG="$DIST/Takeoff-Desk-${VERSION}-${ARCH}.dmg"
# A blank image mounted at a private folder, not -srcfolder: that mounts under
# /Volumes, where macOS refuses to let the terminal read a Developer ID app back.
STAGE="$(mktemp -d)"
mkdir "$STAGE/mnt"
hdiutil create -quiet -size "$(( $(du -sm "$APP" | cut -f1) + 64 ))m" -fs HFS+ -volname "Takeoff Desk" "$STAGE/rw.dmg"
hdiutil attach -quiet -nobrowse -noautoopen -mountpoint "$STAGE/mnt" "$STAGE/rw.dmg"
ditto "$APP" "$STAGE/mnt/$(basename "$APP")"
ln -s /Applications "$STAGE/mnt/Applications"
hdiutil detach -quiet -force "$STAGE/mnt"
rm -f "$DMG"
hdiutil convert -quiet "$STAGE/rw.dmg" -format UDZO -o "$DMG"
rm -rf "$STAGE"
[ -n "${SIGN_IDENTITY:-}" ] && codesign --force --timestamp --sign "$SIGN_IDENTITY" "$DMG"

if [ "${NOTARIZE:-0}" = "1" ]; then
	echo "› notarizing"
	xcrun notarytool submit "$DMG" --apple-id "$APPLE_ID" --team-id "$APPLE_TEAM_ID" --password "$APPLE_APP_SPECIFIC_PASSWORD" --wait
	xcrun stapler staple "$DMG"
fi
echo "✓ $DMG ($(du -h "$DMG" | cut -f1))"
