#!/usr/bin/env bash
#
# Build AdiDNS.app (a menu-bar app bundling the adi-dns resolver) and package a
# DMG. No Xcode project — the .app is assembled by hand and the Swift sources are
# compiled with swiftc, so the whole thing is one scriptable step.
#
# Output:  build/AdiDNS.app  and  build/AdiDNS.dmg
#
# Requirements: Xcode command-line toolchain (swiftc), cargo, codesign, hdiutil.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

APP_NAME="ADI"
DEPLOY_TARGET="13.0"
BUILD="$SCRIPT_DIR/build"
APP="$BUILD/$APP_NAME.app"
ARCH="$(uname -m)"  # arm64 (Apple Silicon) or x86_64 (Intel)

echo "==> building adi-dns (release)"
( cd "$ROOT" && cargo build -p adi-dns --release )
DNS_BIN="$ROOT/target/release/adi-dns"
[ -x "$DNS_BIN" ] || { echo "error: $DNS_BIN missing"; exit 1; }

echo "==> assembling $APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$SCRIPT_DIR/Info.plist" "$APP/Contents/Info.plist"
cp "$DNS_BIN" "$APP/Contents/Resources/adi-dns"

echo "==> compiling Swift ($ARCH-apple-macos$DEPLOY_TARGET)"
swiftc -parse-as-library -O \
    -target "${ARCH}-apple-macos${DEPLOY_TARGET}" \
    -o "$APP/Contents/MacOS/$APP_NAME" \
    "$SCRIPT_DIR"/Sources/*.swift

# Sign nested Mach-O first, then the bundle. With SIGN_ID set (a "Developer ID
# Application" identity) we sign for distribution: hardened runtime + secure
# timestamp, which notarization requires. Without it, ad-hoc — fine for local use.
if [ -n "${SIGN_ID:-}" ]; then
    echo "==> codesign (Developer ID: $SIGN_ID)"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP/Contents/Resources/adi-dns"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP"
    codesign --verify --strict --verbose=2 "$APP"
else
    echo "==> ad-hoc codesign (set SIGN_ID=\"Developer ID Application: …\" for a distributable build)"
    codesign --force --sign - --timestamp=none "$APP/Contents/Resources/adi-dns"
    codesign --force --sign - --timestamp=none "$APP"
fi

echo "==> building DMG"
DMGROOT="$BUILD/dmgroot"
rm -rf "$DMGROOT"
mkdir -p "$DMGROOT"
cp -R "$APP" "$DMGROOT/"
ln -s /Applications "$DMGROOT/Applications"
rm -f "$BUILD/$APP_NAME.dmg"
hdiutil create -volname "$APP_NAME" -srcfolder "$DMGROOT" \
    -ov -format UDZO "$BUILD/$APP_NAME.dmg" >/dev/null
rm -rf "$DMGROOT"

echo
echo "==> done"
echo "    app: $APP"
echo "    dmg: $BUILD/$APP_NAME.dmg"
