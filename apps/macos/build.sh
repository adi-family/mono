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
ICNS="$SCRIPT_DIR/$APP_NAME.icns"

# Build a universal (arm64 + x86_64) app so it runs on both Apple Silicon and
# Intel Macs. A single-arch build fails to launch on the other arch with
# "bad CPU type in executable". Rust builds per-triple, Swift per-arch, then
# `lipo` fuses each Mach-O into one fat binary.
RUST_TARGETS=(aarch64-apple-darwin x86_64-apple-darwin)
SWIFT_ARCHES=(arm64 x86_64)

# Regenerate the app icon from icon-gen.swift (matches Sources/ADILogo.swift), then exit.
if [ "${1:-}" = "--regen-icon" ]; then
    echo "==> regenerating $APP_NAME.icns"
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    swift "$SCRIPT_DIR/icon-gen.swift" "$TMP/icon_1024.png"
    mkdir -p "$TMP/$APP_NAME.iconset"
    for s in 16 32 128 256 512; do
        sips -z "$s" "$s" "$TMP/icon_1024.png" --out "$TMP/$APP_NAME.iconset/icon_${s}x${s}.png" >/dev/null
        sips -z "$((s*2))" "$((s*2))" "$TMP/icon_1024.png" --out "$TMP/$APP_NAME.iconset/icon_${s}x${s}@2x.png" >/dev/null
    done
    iconutil -c icns "$TMP/$APP_NAME.iconset" -o "$ICNS"
    echo "    wrote $ICNS"
    exit 0
fi

echo "==> building adi-dns + adi-hive + adi-app + adi-mono (release, universal: ${RUST_TARGETS[*]})"
( cd "$ROOT" && MACOSX_DEPLOYMENT_TARGET="$DEPLOY_TARGET" cargo build \
    -p adi-dns -p adi-hive -p adi-app -p adi-cli --release \
    "${RUST_TARGETS[@]/#/--target=}" )
# Each --target=<triple> lands its output in target/<triple>/release/, so verify
# every binary exists for every arch before we lipo them together.
for name in adi-dns adi-hive adi-app adi-mono; do
    for t in "${RUST_TARGETS[@]}"; do
        [ -x "$ROOT/target/$t/release/$name" ] || { echo "error: $ROOT/target/$t/release/$name missing"; exit 1; }
    done
done

echo "==> assembling $APP_NAME.app"
rm -rf "$APP"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp "$SCRIPT_DIR/Info.plist" "$APP/Contents/Info.plist"
# Stamp the bundle version from the workspace version — the single source of truth in
# the root Cargo.toml. The auto-updater compares the installed app's Info.plist against
# the published manifest, so the two must always be derived from the same number.
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n1)"
[ -n "$VERSION" ] || { echo "error: workspace version not found in $ROOT/Cargo.toml" >&2; exit 1; }
plutil -replace CFBundleShortVersionString -string "$VERSION" "$APP/Contents/Info.plist"
plutil -replace CFBundleVersion -string "$VERSION" "$APP/Contents/Info.plist"
echo "    version: $VERSION"
# App icon (Info.plist references it via CFBundleIconFile = ADI). Regenerate with
# `build.sh --regen-icon`.
[ -f "$ICNS" ] && cp "$ICNS" "$APP/Contents/Resources/$APP_NAME.icns"
# adi-mono resolves adi-dns/adi-hive/adi-app as siblings, so they all live side by side
# in Resources (adi-hive runs adi-app as the app.adi front-door service). Fuse the
# per-arch builds into one universal Mach-O each.
for name in adi-dns adi-hive adi-app adi-mono; do
    srcs=(); for t in "${RUST_TARGETS[@]}"; do srcs+=("$ROOT/target/$t/release/$name"); done
    lipo -create "${srcs[@]}" -output "$APP/Contents/Resources/$name"
done

echo "==> compiling Swift (universal: ${SWIFT_ARCHES[*]}, macos$DEPLOY_TARGET)"
SWIFT_TMP="$(mktemp -d)"
trap 'rm -rf "$SWIFT_TMP"' EXIT
swift_slices=()
for a in "${SWIFT_ARCHES[@]}"; do
    swiftc -parse-as-library -O \
        -target "${a}-apple-macos${DEPLOY_TARGET}" \
        -o "$SWIFT_TMP/$APP_NAME-$a" \
        "$SCRIPT_DIR"/Sources/*.swift
    swift_slices+=("$SWIFT_TMP/$APP_NAME-$a")
done
lipo -create "${swift_slices[@]}" -output "$APP/Contents/MacOS/$APP_NAME"

# Sign nested Mach-O first, then the bundle. With SIGN_ID set (a "Developer ID
# Application" identity) we sign for distribution: hardened runtime + secure
# timestamp, which notarization requires. Without it, ad-hoc — fine for local use.
if [ -n "${SIGN_ID:-}" ]; then
    echo "==> codesign (Developer ID: $SIGN_ID)"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP/Contents/Resources/adi-dns"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP/Contents/Resources/adi-hive"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP/Contents/Resources/adi-app"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP/Contents/Resources/adi-mono"
    codesign --force --options runtime --timestamp --sign "$SIGN_ID" "$APP"
    codesign --verify --strict --verbose=2 "$APP"
else
    echo "==> ad-hoc codesign (set SIGN_ID=\"Developer ID Application: …\" for a distributable build)"
    codesign --force --sign - --timestamp=none "$APP/Contents/Resources/adi-dns"
    codesign --force --sign - --timestamp=none "$APP/Contents/Resources/adi-hive"
    codesign --force --sign - --timestamp=none "$APP/Contents/Resources/adi-app"
    codesign --force --sign - --timestamp=none "$APP/Contents/Resources/adi-mono"
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
