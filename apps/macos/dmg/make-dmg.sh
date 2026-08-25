#!/usr/bin/env bash
#
# Package a designed ADI.dmg around an already-built, already-signed ADI.app.
#
#   make-dmg.sh <path/to/ADI.app> <path/to/out.dmg>
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
# The names and positions here are load-bearing: layout.DS_Store stores the icon positions
# against the item names "ADI.app" and "Applications", and the background against the path
# ".background/background.tiff". Rename any of them and the window opens unstyled, with no
# error anywhere.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP="${1:?usage: make-dmg.sh <ADI.app> <out.dmg>}"
OUT="${2:?usage: make-dmg.sh <ADI.app> <out.dmg>}"
VOLNAME="ADI"

[ -d "$APP" ] || { echo "error: no app bundle at $APP" >&2; exit 1; }
for asset in background.tiff layout.DS_Store; do
    [ -f "$HERE/$asset" ] || { echo "error: missing $HERE/$asset — run make-assets.sh" >&2; exit 1; }
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
cp "$HERE/background.tiff" "$ROOT/.background/background.tiff"
cp "$HERE/layout.DS_Store" "$ROOT/.DS_Store"
cp "$HERE/../$VOLNAME.icns" "$ROOT/.VolumeIcon.icns"

# Two stages, because the "this volume has a custom icon" bit lives in the volume root's
# Finder flags, which only exist once there is a mounted volume to set them on. -nobrowse
# keeps Finder from adopting the window and rewriting the .DS_Store we just placed.
hdiutil create -volname "$VOLNAME" -srcfolder "$ROOT" -fs HFS+ \
    -format UDRW -ov "$TMP/rw.dmg" >/dev/null
MOUNT="$(hdiutil attach "$TMP/rw.dmg" -readwrite -noverify -nobrowse -noautoopen \
         | grep -o '/Volumes/.*' | head -1)"
[ -n "$MOUNT" ] || { echo "error: could not mount the staging image" >&2; exit 1; }
SetFile -a C "$MOUNT"
sync
hdiutil detach "$MOUNT" >/dev/null
MOUNT=""

rm -f "$OUT"
hdiutil convert "$TMP/rw.dmg" -format UDZO -imagekey zlib-level=9 -o "$OUT" >/dev/null
