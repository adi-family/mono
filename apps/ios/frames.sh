#!/usr/bin/env bash
#
# Compose the App Store panels from the raw captures.
#
#   apps/ios/frames.sh                 # both devices
#   apps/ios/frames.sh --device ipad
#
# In:  apps/ios/shots/<device>/NN-name.png     raw captures from shots.sh
# Out: apps/ios/shots/<device>/store/NN-*.png  the panels that go on the listing
#
# ## Why Chrome and not Pillow
#
# The composition is `frames/panel.html` + `frames/_frame.css`, and those implement
# `projects/adi-design/decisions/2026-08-27-dimensional-not-generated.md` — which states its
# permitted values as CSS: five blurred sources at named alphas, a radial vignette, an
# feTurbulence grain at .062 under the readable layers. A browser is the thing that evaluates
# those the same way the reference render did. The first version of these panels was composited
# in Pillow from two hex codes and a guess, and it looked like it.
#
# ## The Chrome gotcha, which costs an hour to rediscover
#
# On the Chrome installed here, `--headless=new --screenshot` writes the PNG and then does not
# exit — with or without --virtual-time-budget. A plain invocation in a loop never reaches the
# second render. So: launch it, wait for the file to appear and stop growing, kill it, and kill
# the helpers by their unique profile path. Lifted from
# `projects/adi-design/explorations/04-dimensional/render.sh`, which learned it first.
#
# Rendered at 2x and downscaled with LANCZOS, so the delivered pixels are averages of rendered
# ones — the same reason the reference does it.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
DEVICE=both

while [[ $# -gt 0 ]]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2 ;;
    -h|--help) sed -n '2,12p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'; exit 0 ;;
    *) echo "error: unknown argument: $1" >&2; exit 2 ;;
  esac
done

[ -x "$CHROME" ] || { echo "error: Chrome not found at $CHROME (set CHROME=…)" >&2; exit 1; }

log() { printf '\033[1;34m==>\033[0m %s\n' "$*" >&2; }

# name|headline|subhead   — the panel order is the order they appear on the listing.
#
# Four, not five. `03-scan` is the scanner, which is the app's primary pairing action — and it is
# NOT here, because a simulator has no camera and what it captures is the fallback state, not a
# viewfinder. A panel showing "this device has no camera" is worse than no panel. The shot that
# belongs here has to come off a real phone pointed at a real code; until then panel 3 shows the
# invite QR, and its copy says what that picture actually is rather than claiming a scan.
PANELS=(
  "04-fleet|Every machine you run,
in your pocket|Pair once, and they are all just there."
  "05-dashboard|Open its dashboards.
Live, from anywhere.|That clock is the server's, not the phone's."
  "02-invite|A code pairs it.
Nothing else.|No sign-up, no cloud, nothing to forget."
  "01-empty|No account.
No open ports.|Reached by key, encrypted end to end."
)

urlencode() { python3 -c 'import sys,urllib.parse; print(urllib.parse.quote(sys.argv[1], safe=""))' "$1"; }

render() {
  local url="$1" out="$2" w="$3" h="$4" profile size=0 prev=-1 waited=0 pid
  profile="$(mktemp -d)"
  rm -f "$out"
  "$CHROME" --headless=new --disable-gpu --hide-scrollbars --no-first-run \
      --allow-file-access-from-files --user-data-dir="$profile" \
      --force-device-scale-factor=2 --window-size="$w,$h" \
      --screenshot="$out" "$url" >/dev/null 2>&1 &
  pid=$!
  while [ "$waited" -lt 90 ]; do
    sleep .5; waited=$((waited + 1))
    size=$(stat -f %z "$out" 2>/dev/null || echo 0)
    [ "$size" -gt 0 ] && [ "$size" = "$prev" ] && break
    prev=$size
  done
  kill "$pid" 2>/dev/null || true; wait "$pid" 2>/dev/null || true
  # Killing the launcher does not take its helpers with it, and a survivor will rewrite the PNG
  # we are about to downscale. The profile path identifies exactly this render's processes.
  pkill -9 -f "$profile" 2>/dev/null || true
  rm -rf "$profile"
  [ "${size:-0}" -gt 0 ] || { echo "   FAILED: nothing written to $out" >&2; return 1; }
}

for dev in iphone ipad; do
  [ "$DEVICE" = both ] || [ "$DEVICE" = "$dev" ] || continue
  src="$here/shots/$dev"
  out="$src/store"
  plates="$out/_plate"
  [ -d "$src" ] || { echo "skip $dev: no raw captures — run shots.sh --device $dev" >&2; continue; }
  mkdir -p "$out" "$plates"

  # The panel is the size of the capture, because those are already the sizes App Store Connect
  # accepts. Resizing a screenshot to fit a frame is how a set gets rejected for wrong dimensions.
  read -r W H < <(python3 -c "
from PIL import Image
import glob, sys
p = sorted(glob.glob('$src/*.png'))[0]
im = Image.open(p); print(im.size[0], im.size[1])")

  # The phone is narrower on the iPad panel: a 13\" panel is nearly square, so a device at the
  # iPhone panel's fraction would be enormous and the copy would have nowhere to sit.
  case "$dev" in
    iphone) DW=0.94; KIND=phone;  TB=$W;                  ST=0.205; BL=.092; VG=.38 ;;
    ipad)   DW=0.76; KIND=tablet; TB=$(( W * 68 / 100 )); ST=0.265; BL=.092; VG=.38 ;;
  esac

  log "$dev — ${W}x${H}, device at ${DW} of the frame"
  i=0
  for spec in "${PANELS[@]}"; do
    i=$((i + 1))
    name="${spec%%|*}"; rest="${spec#*|}"
    head="${rest%%|*}"; sub="${rest#*|}"
    shot="$src/$name.png"
    [ -f "$shot" ] || { echo "   skip $name: missing" >&2; continue; }

    read -r SW SH < <(python3 -c "
from PIL import Image
im = Image.open('$shot'); print(im.size[0], im.size[1])")

    url="file://$here/frames/panel.html?w=$W&h=$H&sw=$SW&sh=$SH&dw=$DW&kind=$KIND&tb=$TB&st=$ST&bl=$BL&vg=$VG"
    url+="&shot=$(urlencode "$shot")"
    url+="&head=$(urlencode "$head")&sub=$(urlencode "$sub")"

    dest="$out/$(printf '%02d' "$i")-${name#*-}.png"
    render "$url" "$dest.2x.png" "$W" "$H"
    # The same composition with the copy hidden. `frames/measure.py` reads the background
    # luminance off this; measured on the full render, the brightest thing beside a glyph is
    # another glyph's antialiasing and the contrast number describes the type, not the ground.
    plate="$plates/$(basename "$dest")"
    render "$url&mode=plate" "$plate.2x.png" "$W" "$H"
    # Plates live in their own directory so that `store/*.png` is exactly the set that gets
    # uploaded — a measurement artefact sitting beside the deliverables is one somebody submits.
    python3 -c "
from PIL import Image
for a, b in (('$dest.2x.png', '$dest'), ('$plate.2x.png', '$plate')):
    Image.open(a).convert('RGB').resize(($W, $H), Image.LANCZOS).save(b)"
    rm -f "$dest.2x.png" "$plate.2x.png"
    printf '    %s  %sx%s\n' "$(basename "$dest")" "$W" "$H" >&2
  done

  # Measured, not asserted. The design decision this implements says the number has to be
  # reported, and the geometry flags are passed here so nobody has to remember them — a check
  # that needs an argument you can get wrong is a check that quietly measures the wrong thing.
  python3 "$here/frames/measure.py" --dw "$DW" --st "$ST" "$out"/[0-9]*.png >&2
done
