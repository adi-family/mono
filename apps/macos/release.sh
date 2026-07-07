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

APP="$SCRIPT_DIR/build/$APP_NAME.app"

if [ -n "${AC_USER:-}" ] && [ -n "${AC_PASS:-}" ]; then
    # Staple the notarization ticket to the .app — not only the DMG. Users drag the app OUT
    # of the DMG into /Applications, so a ticket on the DMG alone leaves the *installed* app
    # depending on a fragile online Gatekeeper check. On another Mac that surfaces as
    # "-10810 / Launchd job spawn failed" when the check can't complete (offline, flaky) or
    # the signature was nicked in transit. So: notarize + staple the APP first, repackage the
    # DMG around the stapled app, then notarize + staple the DMG too (offline-safe download).
    echo "==> notarizing the app (uploads to Apple; waits for the result)"
    APPZIP="$SCRIPT_DIR/build/$APP_NAME-app.zip"
    ditto -c -k --keepParent "$APP" "$APPZIP"
    xcrun notarytool submit "$APPZIP" \
        --apple-id "$AC_USER" --team-id "$TEAM_ID" --password "$AC_PASS" --wait
    rm -f "$APPZIP"
    echo "==> stapling the app"
    xcrun stapler staple "$APP"

    echo "==> repackaging the DMG around the stapled app"
    DMGROOT="$SCRIPT_DIR/build/dmgroot"
    rm -rf "$DMGROOT"; mkdir -p "$DMGROOT"
    cp -R "$APP" "$DMGROOT/"
    ln -s /Applications "$DMGROOT/Applications"
    rm -f "$DMG"
    hdiutil create -volname "$APP_NAME" -srcfolder "$DMGROOT" -ov -format UDZO "$DMG" >/dev/null
    rm -rf "$DMGROOT"
    codesign --force --sign "$SIGN_ID" --timestamp "$DMG"

    echo "==> notarizing the DMG (uploads to Apple; waits for the result)"
    xcrun notarytool submit "$DMG" \
        --apple-id "$AC_USER" --team-id "$TEAM_ID" --password "$AC_PASS" --wait
    echo "==> stapling the DMG"
    xcrun stapler staple "$DMG"

    echo "✓ notarized + stapled (app AND dmg): $DMG"
    echo "==> verifying"
    xcrun stapler validate "$APP"
    xcrun stapler validate "$DMG"
    spctl -a -t open --context context:primary-signature -v "$DMG" || true
else
    echo "==> signing the DMG (no notarization — AC_USER/AC_PASS unset)"
    codesign --force --sign "$SIGN_ID" --timestamp "$DMG"
    echo "⚠ DMG is signed but NOT notarized/stapled — it will fail Gatekeeper on other Macs."
    echo "  Set AC_USER/AC_PASS in apps/macos/.env and re-run to notarize + staple."
fi
