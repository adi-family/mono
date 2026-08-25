#!/usr/bin/env bash
#
# Regenerate the disk image's design assets. Developer tool, not part of a release build:
# build.sh only ever copies what this script commits.
#
#   make-assets.sh                              # re-render the release background
#   make-assets.sh --bake-layout                # ...and re-bake its layout by driving Finder
#   make-assets.sh --flavor dev --bake-layout   # the same pair for ADI Dev.app
#
# Assets are per flavour and committed: background-<id>.tiff and layout-<id>.DS_Store. The
# layout has to be, not just the art -- Finder keys an icon position on the item's NAME, and a
# dev build ships "ADI Dev.app", so the release layout would leave its icon unplaced.
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
ROOT="$(cd "$HERE/../../.." && pwd)"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
W=680; H=440

FLAVOR="release"
BAKE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --flavor) FLAVOR="${2:?--flavor needs an id}"; shift 2 ;;
        --bake-layout) BAKE=1; shift ;;
        *) echo "usage: make-assets.sh [--flavor <id>] [--bake-layout]" >&2; exit 1 ;;
    esac
done

# The identity comes from the CLI, the same place build.sh gets it, so the art can never
# announce a name the bundle does not ship under.
CLI="$ROOT/target/release/adi-mono"
[ -x "$CLI" ] || CLI="$ROOT/target/debug/adi-mono"
[ -x "$CLI" ] || { echo "error: build adi-mono first (cargo build -p adi-cli)" >&2; exit 1; }
eval "$("$CLI" --flavor "$FLAVOR" flavor --env)"
APP_NAME="$ADI_APP_NAME"
VOLNAME="$APP_NAME"
# The release build's chip says what the binary is; every other flavour spends it on saying
# which install this is, which is the more useful fact when two disk images sit side by side.
if [ "$ADI_FLAVOR" = "release" ]; then CHIP="Universal"; else CHIP="$(echo "$ADI_FLAVOR" | tr '[:lower:]' '[:upper:]')"; fi
BG="$HERE/background-$ADI_FLAVOR.tiff"
LAYOUT="$HERE/layout-$ADI_FLAVOR.DS_Store"
echo "==> flavour $ADI_FLAVOR ($APP_NAME)"

[ -x "$CHROME" ] || { echo "error: Chrome not found at $CHROME (set CHROME=…)" >&2; exit 1; }

TMP="$(mktemp -d)"; trap 'rm -rf "$TMP"' EXIT

echo "==> rendering background.html at 1x and 2x"
QUERY="name=$(printf %s "$APP_NAME" | sed 's/ /%20/g')&chip=$CHIP"
for scale in 1 2; do
    "$CHROME" --headless=new --disable-gpu --hide-scrollbars \
        --force-device-scale-factor="$scale" --window-size="$W,$H" \
        --screenshot="$TMP/bg@${scale}x.png" "file://$HERE/background.html?$QUERY" >/dev/null 2>&1
done

# Refuse to ship art whose label band fails: the flat colour being right is not evidence
# that the rendered pixels are, and a texture or gradient laid over it can break it.
echo "==> checking the readability guardrail"
python3 "$HERE/check-contrast.py" "$TMP/bg@1x.png" "$TMP/bg@2x.png"

echo "==> packing the hidpi TIFF"
tiffutil -cathidpicheck "$TMP/bg@1x.png" "$TMP/bg@2x.png" -out "$BG" >/dev/null
echo "    wrote $BG"

[ -n "$BAKE" ] || exit 0

# ── layout.DS_Store ──────────────────────────────────────────────────────────────────
#
# Finder is the only thing that writes a .DS_Store, so the layout is baked once here by
# driving it over AppleScript against a scratch read-write image, then committed. The
# release build just copies the result. Doing this at build time instead would put Finder
# automation on the CI runner's critical path, where it is both slow and flaky.
echo "==> baking layout.DS_Store"
STAGE="$TMP/root"; mkdir -p "$STAGE/.background"
cp "$BG" "$STAGE/.background/background.tiff"
# Finder will only place an item that exists, and it keys the position on the name — so the
# stand-in has to be called exactly what ships.
mkdir -p "$STAGE/$APP_NAME.app"
ln -s /Applications "$STAGE/Applications"

hdiutil create -volname "$VOLNAME" -srcfolder "$STAGE" -fs HFS+ \
    -format UDRW -ov "$TMP/scratch.dmg" >/dev/null
MOUNT="$(hdiutil attach "$TMP/scratch.dmg" -readwrite -noverify -noautoopen \
         | grep -o '/Volumes/.*' | head -1)"
[ -n "$MOUNT" ] || { echo "error: could not mount the scratch image" >&2; exit 1; }
trap 'hdiutil detach "$MOUNT" -force >/dev/null 2>&1 || true; rm -rf "$TMP"' EXIT

# Drive whatever the volume actually mounted AS, not what we asked for. With a built
# ADI.dmg already mounted, the scratch lands on "/Volumes/ADI 1" and addressing disk "ADI"
# by name silently applies the layout to the read-only image instead -- which fails only
# later, at the copy, with the .DS_Store never written.
osascript "$HERE/layout.applescript" "$(basename "$MOUNT")" "$APP_NAME.app" >/dev/null

sync; sleep 1
cp "$MOUNT/.DS_Store" "$LAYOUT"
hdiutil detach "$MOUNT" >/dev/null
trap 'rm -rf "$TMP"' EXIT
echo "    wrote $LAYOUT"
