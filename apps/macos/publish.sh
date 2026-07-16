#!/usr/bin/env bash
#
# Publish a release for the auto-updater: build the notarized DMG (release.sh), write
# build/manifest.json (version / url / sha256 — what `adi-mono update` polls), and
# upload both as a GitHub release. The client-side default manifest URL is
#   https://github.com/<repo>/releases/latest/download/manifest.json
# so publishing a new release is all it takes for every machine to pick it up.
#
# Usage:
#   apps/macos/publish.sh                 # release.sh + manifest + gh release
#   apps/macos/publish.sh --skip-build    # reuse build/ADI.dmg from a previous run
#   apps/macos/publish.sh --no-upload     # stop after writing build/manifest.json
#   apps/macos/publish.sh --notes "..."   # release notes (also embedded in the manifest)
#
# Env:
#   ADI_UPDATE_REPO  GitHub repo the release goes to (default: mgorunuch/adi-family)
#
# Uploading needs the `gh` CLI, authenticated (`gh auth login`), and the repo to exist.
# With --no-upload the manifest + DMG in build/ are ready for any static host instead —
# just keep the manifest's `dmg.url` pointing where the DMG will actually live.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="ADI"
BUILD="$SCRIPT_DIR/build"
APP="$BUILD/$APP_NAME.app"
DMG="$BUILD/$APP_NAME.dmg"
MANIFEST="$BUILD/manifest.json"

# Same credential auto-load as release.sh, so this can run standalone.
if [ -f "$SCRIPT_DIR/.env" ]; then set -a; . "$SCRIPT_DIR/.env"; set +a; fi

REPO="${ADI_UPDATE_REPO:-mgorunuch/adi-family}"
SKIP_BUILD=false
UPLOAD=true
NOTES=""
while [ $# -gt 0 ]; do
    case "$1" in
        --skip-build) SKIP_BUILD=true ;;
        --no-upload)  UPLOAD=false ;;
        --notes)      NOTES="${2:?--notes needs a value}"; shift ;;
        *) echo "error: unknown flag $1" >&2; exit 2 ;;
    esac
    shift
done

if ! $SKIP_BUILD; then
    "$SCRIPT_DIR/release.sh"
fi
[ -f "$DMG" ] || { echo "error: $DMG missing — run without --skip-build" >&2; exit 1; }
[ -d "$APP" ] || { echo "error: $APP missing — run without --skip-build" >&2; exit 1; }

VERSION="$(plutil -extract CFBundleShortVersionString raw -o - "$APP/Contents/Info.plist")"
[ -n "$VERSION" ] || { echo "error: could not read the app version" >&2; exit 1; }
SHA256="$(shasum -a 256 "$DMG" | awk '{print $1}')"
SIZE="$(stat -f%z "$DMG")"
PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
URL="https://github.com/$REPO/releases/download/v$VERSION/$APP_NAME.dmg"

# The updater rejects unstapled bundles only via Gatekeeper on first manual install;
# still, warn early rather than shipping a build that will fail on other Macs.
xcrun stapler validate "$DMG" >/dev/null 2>&1 \
    || echo "⚠ $DMG is not notarized/stapled — fine for testing, not for distribution."

echo "==> writing $MANIFEST (v$VERSION)"
NOTES_JSON="$(printf '%s' "${NOTES:-ADI v$VERSION}" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g')"
cat > "$MANIFEST" <<EOF
{
  "version": "$VERSION",
  "pub_date": "$PUB_DATE",
  "notes": "$NOTES_JSON",
  "dmg": {
    "url": "$URL",
    "sha256": "$SHA256",
    "size": $SIZE
  }
}
EOF
cat "$MANIFEST"

if ! $UPLOAD; then
    echo "==> --no-upload: done. Host $DMG at the manifest's dmg.url and serve $MANIFEST."
    exit 0
fi

command -v gh >/dev/null 2>&1 || {
    echo "error: the GitHub CLI (gh) is required to upload — brew install gh && gh auth login" >&2
    echo "       (or re-run with --no-upload and host build/ yourself)" >&2
    exit 1
}

echo "==> creating GitHub release v$VERSION on $REPO"
gh release create "v$VERSION" --repo "$REPO" \
    --title "$APP_NAME v$VERSION" \
    --notes "${NOTES:-$APP_NAME v$VERSION}" \
    --latest \
    "$DMG" "$MANIFEST"

echo "✓ published: https://github.com/$REPO/releases/tag/v$VERSION"
echo "  clients poll: https://github.com/$REPO/releases/latest/download/manifest.json"
