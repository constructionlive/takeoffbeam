#!/usr/bin/env bash
# Publishes a release to the update feed every installed copy checks:
#   https://pub-061018a047d54079b309fb16a26a1ce0.r2.dev/takeoff-desk/latest.json
# Uploads dist/ installers for this version + latest.json to Cloudflare R2.
#
#   NOTES="- Faster rendering\n- New Follow Line tool" scripts/publish-update.sh
#   CRITICAL=1 …    # can't be skipped by users
#   DRY_RUN=1 …     # print the manifest, upload nothing
# Credentials: AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY (an R2 token with Object
# Read & Write on the bucket), e.g. `set -a; source …/electron-builder.env; set +a`.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
VERSION="$(grep -m1 '^version' Cargo.toml | sed -E 's/.*"(.*)".*/\1/')"
ENDPOINT="${R2_ENDPOINT:-https://136b5efccdb1fd7296a5546ab0a6b41f.r2.cloudflarestorage.com}"
BUCKET="${R2_BUCKET:-construction-live-desktop}"
PUBLIC="${R2_PUBLIC:-https://pub-061018a047d54079b309fb16a26a1ce0.r2.dev}"
PREFIX="takeoff-desk"

MAC="dist/Takeoff-Desk-${VERSION}-arm64.dmg"
WIN="dist/Takeoff-Desk-${VERSION}-x64-setup.exe"
[ -f "$MAC" ] || [ -f "$WIN" ] || { echo "no installers for $VERSION in dist/ — run the package scripts first" >&2; exit 1; }

upload() { # file key content-type
	if [ "${DRY_RUN:-0}" = "1" ]; then echo "  (dry run) $1 → $2"; return; fi
	curl -sSf --aws-sigv4 "aws:amz:auto:s3" --user "$AWS_ACCESS_KEY_ID:$AWS_SECRET_ACCESS_KEY" \
		-H "Content-Type: $3" -H "Cache-Control: ${4:-public, max-age=31536000}" \
		-T "$1" "$ENDPOINT/$BUCKET/$2" >/dev/null
	echo "  ✓ $PUBLIC/$2"
}

echo "› publishing Takeoff Desk $VERSION"
DOWNLOADS="{"
if [ -f "$MAC" ]; then
	upload "$MAC" "$PREFIX/$VERSION/$(basename "$MAC")" "application/x-apple-diskimage"
	DOWNLOADS+="\"macos-arm64\": \"$PUBLIC/$PREFIX/$VERSION/$(basename "$MAC")\""
fi
if [ -f "$WIN" ]; then
	upload "$WIN" "$PREFIX/$VERSION/$(basename "$WIN")" "application/vnd.microsoft.portable-executable"
	[ "$DOWNLOADS" != "{" ] && DOWNLOADS+=", "
	DOWNLOADS+="\"windows-x64\": \"$PUBLIC/$PREFIX/$VERSION/$(basename "$WIN")\""
fi
DOWNLOADS+="}"

# Stable "latest" links for websites and emails (they never change between releases).
[ -f "$MAC" ] && upload "$MAC" "$PREFIX/latest/Takeoff-Desk-arm64.dmg" "application/x-apple-diskimage" "public, max-age=300"
[ -f "$WIN" ] && upload "$WIN" "$PREFIX/latest/Takeoff-Desk-x64-setup.exe" "application/vnd.microsoft.portable-executable" "public, max-age=300"
VIDEO="docs/media/Takeoff-Desk-launch.mp4"
[ -f "$VIDEO" ] && upload "$VIDEO" "$PREFIX/latest/Takeoff-Desk-launch.mp4" "video/mp4" "public, max-age=3600"

MANIFEST="$(mktemp)"
NOTES="${NOTES:-}" VERSION="$VERSION" CRITICAL="${CRITICAL:-0}" DOWNLOADS="$DOWNLOADS" python3 - > "$MANIFEST" <<'PY'
import json, os, datetime
print(json.dumps({
	"version": os.environ["VERSION"],
	"date": datetime.date.today().isoformat(),
	"notes": os.environ["NOTES"].replace("\\n", "\n"),
	"critical": os.environ["CRITICAL"] == "1",
	"downloads": json.loads(os.environ["DOWNLOADS"]),
	"page": "https://github.com/constructionlive/takeoffbeam/releases",
}, indent=2))
PY
cat "$MANIFEST"
# The manifest must not be cached long, or clients won't see the update.
upload "$MANIFEST" "$PREFIX/latest.json" "application/json" "public, max-age=300"
echo "✓ every copy of Takeoff Desk older than $VERSION will offer this update within 6 hours (or at next launch)"
