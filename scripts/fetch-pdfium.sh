#!/usr/bin/env bash
# Downloads the prebuilt pdfium matching the pdfium-render API level we build
# against (chromium/7881) into vendor/pdfium. Binaries: github.com/bblanchon/pdfium-binaries
set -euo pipefail
PDFIUM_TAG="chromium%2F7881"
case "$(uname -s)-$(uname -m)" in
	Darwin-arm64) PKG=pdfium-mac-arm64 ;;
	Darwin-x86_64) PKG=pdfium-mac-x64 ;;
	Linux-x86_64) PKG=pdfium-linux-x64 ;;
	Linux-aarch64) PKG=pdfium-linux-arm64 ;;
	*) echo "unsupported platform: $(uname -s)-$(uname -m)" >&2; exit 1 ;;
esac
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
rm -rf "$ROOT/vendor/pdfium" && mkdir -p "$ROOT/vendor/pdfium"
curl -sSLf "https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_TAG}/${PKG}.tgz" \
	| tar xz -C "$ROOT/vendor/pdfium"
echo "pdfium $(grep BUILD "$ROOT/vendor/pdfium/VERSION") -> vendor/pdfium/lib"
