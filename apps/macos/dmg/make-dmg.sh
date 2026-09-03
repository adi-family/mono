#!/usr/bin/env bash
#
# Package a designed ADI.dmg around an already-built, already-signed ADI.app.
#
#   make-dmg.sh <path/to/ADI.app> <path/to/out.dmg> [flavor]
#
# Both build.sh and release.sh call this. release.sh repackages after stapling the app, so
# the two used to carry their own copy of the assembly and were free to drift apart — the
# design lives here instead, in one place.
#
# What goes in beyond the app and the /Applications symlink:
#
#   .DS_Store            the committed Finder layout (window size, icon positions, the
#                        background reference, toolbar and status bar off)
#   .background/         the background picture the layout points at, 1x + 2x in one TIFF
#   .VolumeIcon.icns     the mounted volume's icon, in the Finder sidebar and on the desktop
#
# The names and positions here are load-bearing: the layout stores icon positions against the
# item names ("ADI.app" / "ADI Dev.app", and "Applications") and the background against the
# path ".background/background.tiff". Rename any of them and the window opens unstyled, with no
# error anywhere.
#
# The volume takes its name from the bundle rather than from the flavour argument. They agree,
# but only one of them is the thing Finder will actually see, and deriving it from the bundle
# means a mismatch is impossible rather than merely unlikely.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="${1:?usage: make-dmg.sh <ADI.app> <out.dmg> [flavor]}"
OUT="${2:?usage: make-dmg.sh <ADI.app> <out.dmg> [flavor]}"
FLAVOR="${3:-release}"
VOLNAME="$(basename "$APP" .app)"
BG="$HERE/background-$FLAVOR.tiff"
LAYOUT="$HERE/layout-$FLAVOR.DS_Store"

[ -d "$APP" ] || { echo "error: no app bundle at $APP" >&2; exit 1; }
for asset in "$BG" "$LAYOUT"; do
    [ -f "$asset" ] || {
        echo "error: missing $asset" >&2
        echo "       run: apps/macos/dmg/make-assets.sh --flavor $FLAVOR --bake-layout" >&2
        exit 1
    }
done

TMP="$(mktemp -d)"
MOUNT=""
cleanup() {
    [ -n "$MOUNT" ] && hdiutil detach "$MOUNT" -force >/dev/null 2>&1
    rm -rf "$TMP"
}
trap cleanup EXIT

ROOT="$TMP/root"
mkdir -p "$ROOT/.background"
cp -R "$APP" "$ROOT/$VOLNAME.app"
ln -s /Applications "$ROOT/Applications"
cp "$BG" "$ROOT/.background/background.tiff"
cp "$LAYOUT" "$ROOT/.DS_Store"
# The mark is shared across flavours until the icon work lands.
cp "$HERE/../ADI.icns" "$ROOT/.VolumeIcon.icns"

# A volume already mounted under this name is one way `create` answers "Resource busy", and
# the only one we can clear from here. A stale one is not ours to keep: the name comes from
# the bundle, so anything holding it is a previous run of this script that died mid-flight.
[ -d "/Volumes/$VOLNAME" ] && hdiutil detach "/Volumes/$VOLNAME" -force >/dev/null 2>&1

# Two stages, because the "this volume has a custom icon" bit lives in the volume root's
# Finder flags, which only exist once there is a mounted volume to set them on. -nobrowse
# keeps Finder from adopting the window and rewriting the .DS_Store we just placed.
#
# Retried, because the first stage is flaky on a CI runner in a way it never is on a desktop:
# v1.3.0's macOS job died on "hdiutil: create failed - Resource busy" after ten minutes of
# building and signing, and the same script against the same app makes the image in nine
# seconds locally. Something still had the tree we had just written; a second later it does
# not. Five tries with a growing pause, and the last failure is still a failure.
for attempt in 1 2 3 4 5; do
    if hdiutil create -volname "$VOLNAME" -srcfolder "$ROOT" -fs HFS+ \
           -format UDRW -ov "$TMP/rw.dmg" >/dev/null 2>"$TMP/create.err"; then
        break
    fi
    cat "$TMP/create.err" >&2
    [ "$attempt" = 5 ] && { echo "error: could not create the staging image" >&2; exit 1; }
    echo "    retrying hdiutil create in ${attempt}s ($attempt/5)" >&2
    sleep "$attempt"
done
MOUNT="$(hdiutil attach "$TMP/rw.dmg" -readwrite -noverify -nobrowse -noautoopen \
         | grep -o '/Volumes/.*' | head -1)"
[ -n "$MOUNT" ] || { echo "error: could not mount the staging image" >&2; exit 1; }
SetFile -a C "$MOUNT"
sync
hdiutil detach "$MOUNT" >/dev/null
MOUNT=""

rm -f "$OUT"
hdiutil convert "$TMP/rw.dmg" -format UDZO -imagekey zlib-level=9 -o "$OUT" >/dev/null
