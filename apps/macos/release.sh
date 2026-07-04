#!/usr/bin/env bash
#
# Build a distributable ADI.dmg: Developer ID signature + hardened runtime +
# notarized + stapled, for direct (non-App-Store) distribution.
#
# Credentials: release.sh auto-loads apps/macos/.env (gitignored) if present, or
# takes them from the environment. Copy .env.example to .env and fill it in:
#
#   cp apps/macos/.env.example apps/macos/.env   # then set TEAM_ID/AC_USER/AC_PASS
#   apps/macos/release.sh
#
# Env:
#   TEAM_ID  Apple Developer team ID           (default: 752556J5V6)
#   SIGN_ID  "Developer ID Application: …"      (default: auto-found for TEAM_ID)
#   AC_USER  Apple ID email (notarization)
#   AC_PASS  app-specific password (notarization)
#
# Signing needs only the keychain cert (no secret). Notarization needs AC_USER +
# AC_PASS; if they're unset the DMG is signed but left un-notarized.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_NAME="ADI"
DMG="$SCRIPT_DIR/build/$APP_NAME.dmg"

# Auto-load local credentials (gitignored) if present, so this can run standalone.
if [ -f "$SCRIPT_DIR/.env" ]; then set -a; . "$SCRIPT_DIR/.env"; set +a; fi

TEAM_ID="${TEAM_ID:-752556J5V6}"

SIGN_ID="${SIGN_ID:-$(security find-identity -v -p codesigning \
    | grep -m1 "Developer ID Application.*($TEAM_ID)" \
    | sed -E 's/.*"(.*)"/\1/')}"
[ -n "$SIGN_ID" ] || {
    echo "✗ No 'Developer ID Application' certificate for team $TEAM_ID in the keychain" >&2
    exit 1
}
export SIGN_ID

echo "==> building + Developer ID signing the app"
"$SCRIPT_DIR/build.sh"

echo "==> signing the DMG"
codesign --force --sign "$SIGN_ID" --timestamp "$DMG"

if [ -n "${AC_USER:-}" ] && [ -n "${AC_PASS:-}" ]; then
    echo "==> notarizing (uploads to Apple; waits for the result)"
    xcrun notarytool submit "$DMG" \
        --apple-id "$AC_USER" --team-id "$TEAM_ID" --password "$AC_PASS" \
        --wait
    echo "==> stapling"
    xcrun stapler staple "$DMG"
    echo "✓ notarized + stapled: $DMG"
    echo "  verify: spctl -a -t open --context context:primary-signature -v \"$DMG\""
else
    echo "⚠ AC_USER/AC_PASS not set — DMG is signed but NOT notarized."
    echo "  Source your .env with the Apple credentials and re-run to notarize."
fi
