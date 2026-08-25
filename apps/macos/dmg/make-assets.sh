#!/usr/bin/env bash
#
# Regenerate the disk image's design assets. Developer tool, not part of a release build:
# build.sh only ever copies what this script commits.
#
#   make-assets.sh                 # re-render background.tiff from background.html
#   make-assets.sh --bake-layout   # ...and re-bake layout.DS_Store by driving Finder
#
# background.tiff carries a 1x and a 2x representation in one file (tiffutil
# -cathidpicheck); Finder picks per display, so the window is sharp on Retina without a
# separate asset. There is no light/dark pair: a disk image background is one flat
# picture whatever the system appearance, which is why the art has to satisfy both.
#
# Requirements: Google Chrome (renders the HTML), python3 + Pillow (the contrast check),
# tiffutil. --bake-layout also needs Finder automation permission for your terminal.
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
VOLNAME="ADI"
W=680; H=440

[ -x "$CHROME" ] || { echo "error: Chrome not found at $CHROME (set CHROME=…)" >&2; exit 1; }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

echo "==> rendering background.html at 1x and 2x"
for scale in 1 2; do
    "$CHROME" --headless=new --disable-gpu --hide-scrollbars \
        --force-device-scale-factor="$scale" --window-size="$W,$H" \
        --screenshot="$TMP/bg@${scale}x.png" "file://$HERE/background.html" >/dev/null 2>&1
done

# Refuse to ship art whose label band fails: the flat colour being right is not evidence
# that the rendered pixels are, and a texture or gradient laid over it can break it.
echo "==> checking the readability guardrail"
python3 "$HERE/check-contrast.py" "$TMP/bg@1x.png" "$TMP/bg@2x.png"

echo "==> packing the hidpi TIFF"
tiffutil -cathidpicheck "$TMP/bg@1x.png" "$TMP/bg@2x.png" -out "$HERE/background.tiff" >/dev/null
echo "    wrote $HERE/background.tiff"

[ "${1:-}" = "--bake-layout" ] || exit 0

# ── layout.DS_Store ──────────────────────────────────────────────────────────────────
#
# Finder is the only thing that writes a .DS_Store, so the layout is baked once here by
# driving it over AppleScript against a scratch read-write image, then committed. The
# release build just copies the result. Doing this at build time instead would put Finder
# automation on the CI runner's critical path, where it is both slow and flaky.
echo "==> baking layout.DS_Store"
ROOT="$TMP/root"; mkdir -p "$ROOT/.background"
cp "$HERE/background.tiff" "$ROOT/.background/background.tiff"
# Finder will only place an item that exists, and it keys the position on the name — so the
# stand-in has to be called exactly what ships.
mkdir -p "$ROOT/$VOLNAME.app"
ln -s /Applications "$ROOT/Applications"

hdiutil create -volname "$VOLNAME" -srcfolder "$ROOT" -fs HFS+ \
    -format UDRW -ov "$TMP/scratch.dmg" >/dev/null
MOUNT="$(hdiutil attach "$TMP/scratch.dmg" -readwrite -noverify -noautoopen \
         | grep -o '/Volumes/.*' | head -1)"
[ -n "$MOUNT" ] || { echo "error: could not mount the scratch image" >&2; exit 1; }
trap 'hdiutil detach "$MOUNT" -force >/dev/null 2>&1 || true; rm -rf "$TMP"' EXIT

# Drive whatever the volume actually mounted AS, not what we asked for. With a built
# ADI.dmg already mounted, the scratch lands on "/Volumes/ADI 1" and addressing disk "ADI"
# by name silently applies the layout to the read-only image instead -- which fails only
# later, at the copy, with the .DS_Store never written.
osascript "$HERE/layout.applescript" "$(basename "$MOUNT")" >/dev/null

sync; sleep 1
cp "$MOUNT/.DS_Store" "$HERE/layout.DS_Store"
hdiutil detach "$MOUNT" >/dev/null
trap 'rm -rf "$TMP"' EXIT
echo "    wrote $HERE/layout.DS_Store"
