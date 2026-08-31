#!/usr/bin/env bash
#
# Capture the raw App Store screens: mint a fresh invite on the demo node, put it on the
# simulator's pasteboard, drive the app to each screen with the UI test harness, and pull the PNGs
# out of the result bundle.
#
#   apps/ios/shots.sh                      # iPhone 6.9" (1320x2868)
#   apps/ios/shots.sh --device ipad        # iPad 13"   (2048x2732)
#   apps/ios/shots.sh --no-mint            # reuse whatever is already on the pasteboard
#
# Output: apps/ios/shots/<device>/NN-name.png — raw device captures, at exactly the pixel sizes App
# Store Connect wants. `apps/ios/frames.ts` is what turns them into the marketing frames.
#
# ## Why a UI test and not `simctl`
#
# `simctl` boots, installs and screenshots but cannot tap, so it can only ever photograph the
# launch screen. The harness is `AdiFleetUITests/ScreenshotTests.swift`.
#
# ## The invite is single-use
#
# Each run mints a new one on `adi-demo` (`docs/fleet.md` §8 — the node mints, the phone spends),
# because a spent nonce is refused and the run would then photograph an empty fleet and pass.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"

DEVICE="iphone"
MINT=1
NODE="adi-demo"
ZONE="europe-southwest1-a"
PROJECT="mono-504617"

die() { echo "error: $*" >&2; exit 1; }
log() { printf '\033[1;34m==>\033[0m %s\n' "$*" >&2; }

while [[ $# -gt 0 ]]; do
  case "$1" in
    --device) DEVICE="$2"; shift 2 ;;
    --no-mint) MINT=0; shift ;;
    --node) NODE="$2"; shift 2 ;;
    -h|--help) sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \?//'; exit 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

case "$DEVICE" in
  iphone) SIM="adi-shot-iphone" ;;
  ipad)   SIM="adi-shot-ipad" ;;
  *) die "--device must be iphone or ipad" ;;
esac

udid="$(xcrun simctl list devices -j | python3 -c "
import json,sys
for runtime in json.load(sys.stdin)['devices'].values():
    for d in runtime:
        if d['name'] == '$SIM': print(d['udid']); raise SystemExit
")"
[[ -n "$udid" ]] || die "no simulator called $SIM — create it, or see apps/ios/APPSTORE.md §2.3"

log "booting $SIM"
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b >/dev/null 2>&1 || true

# Start from no app at all. The harness photographs the empty "No nodes yet" state first, and a
# previous run *really paired* — so without this the second run opens on a populated fleet and
# fails on the first assertion, which reads as a broken harness rather than as leftover state.
# Uninstalling also drops the Keychain identity, so each run pairs as a genuinely new device.
log "uninstalling any previous build (the harness starts from an unpaired app)"
xcrun simctl uninstall "$udid" family.adi.fleet 2>/dev/null || true

# The pasteboard is the channel between this script and the app, so it must start empty — a token
# left over from a previous run would be spent instantly and the pairing would race a dead nonce.
printf 'x' | xcrun simctl pbcopy "$udid" 2>/dev/null || true

log "building the core + project"
(cd "$repo" && cargo build --release -p adi-mesh-ffi --target aarch64-apple-ios-sim >/dev/null)
(cd "$here" && xcodegen generate --quiet)

out="$here/shots/$DEVICE"
result="$here/.build/shots-$DEVICE.xcresult"
rm -rf "$result" "$out"
mkdir -p "$out"

log "running the harness (this drives the app and pairs for real)"
# In the background, because the pairing needs BOTH sides and this script is the other one: the app
# mints an invite and copies it, and nothing will spend it unless something outside the test does.
# `mesh join` then runs on the machine, which dials the phone back — the app's default direction,
# and `docs/fleet.md` §8's first row.
xcodebuild test \
  -project "$here/AdiFleet.xcodeproj" \
  -scheme AdiFleetShots \
  -configuration Debug \
  -sdk iphonesimulator \
  -destination "id=$udid" \
  -derivedDataPath "$here/.build" \
  -resultBundlePath "$result" \
  >"$here/.build/shots-$DEVICE.log" 2>&1 &
xcb=$!

if [[ $MINT -eq 1 ]]; then
  log "waiting for the app to copy an invite, then spending it on $NODE"
  token=""
  for _ in $(seq 1 150); do
    sleep 4
    kill -0 "$xcb" 2>/dev/null || break          # the test died; let the wait below report it
    clip="$(xcrun simctl pbpaste "$udid" 2>/dev/null || true)"
    # The button copies the whole `adi-mono mesh join <token>` command, so take the token out.
    token="$(printf '%s' "$clip" | tr ' ' '\n' | grep -m1 '^adi-invite:' || true)"
    [[ -n "$token" ]] && break
  done
  if [[ -n "$token" ]]; then
    log "  got a ${#token}-char invite; running mesh join on $NODE"
    gcloud compute ssh "$NODE" --zone="$ZONE" --project="$PROJECT" --quiet \
      --command "sudo -u adi -i bash -c 'export PATH=/home/adi/.local/adi/bin:\$PATH; adi-mono mesh join \"$token\"'" \
      >>"$here/.build/shots-$DEVICE.log" 2>&1 \
      && log "  the machine dialled back" \
      || log "  mesh join failed — see the log; the harness will time out and say so"
  else
    log "  no invite ever reached the pasteboard; the harness will report why"
  fi
fi

set +e
wait "$xcb"
status=$?
set -e
# NOT a `die` on failure. Every capture is taken *before* the assertion that guards it, so a run
# that fails late still holds the screens it got — and throwing them away means re-running a
# five-minute pairing to see the picture that says why it failed. Extract first; judge after.
if [[ $status -ne 0 ]]; then
  echo "--- the harness failed (exit $status); last 20 lines ---" >&2
  tail -20 "$here/.build/shots-$DEVICE.log" | grep -E "error:|XCTAssert" >&2 || true
  echo "--- extracting whatever it did capture ---" >&2
fi

log "extracting the captures"
# xcresulttool's --legacy flag: Xcode 16+ made `get object` legacy-only, and without it the call
# fails with "This command is deprecated" rather than returning the graph.
python3 - "$result" "$out" <<'PY'
import json, subprocess, sys, pathlib

bundle, out = sys.argv[1], pathlib.Path(sys.argv[2])

def get(ref=None):
    cmd = ["xcrun", "xcresulttool", "get", "object", "--legacy", "--path", bundle, "--format", "json"]
    if ref:
        cmd += ["--id", ref]
    return json.loads(subprocess.check_output(cmd, stderr=subprocess.DEVNULL))

def walk(node, found):
    """Attachments hang off test activities, several levels down and named inconsistently."""
    if isinstance(node, dict):
        if node.get("_type", {}).get("_name") == "ActionTestAttachment":
            name = node.get("name", {}).get("_value")
            ref = node.get("payloadRef", {}).get("id", {}).get("_value")
            if name and ref:
                found.append((name, ref))
        for v in node.values():
            walk(v, found)
    elif isinstance(node, list):
        for v in node:
            walk(v, found)

root = get()
refs = []
walk(root, refs)

# The summaries are behind their own refs; follow every one that looks like a test summary.
seen, queue = set(), []
def collect_refs(node):
    if isinstance(node, dict):
        rid = node.get("id", {}).get("_value")
        if rid and node.get("_type", {}).get("_name", "").endswith("Reference"):
            queue.append(rid)
        for v in node.values():
            collect_refs(v)
    elif isinstance(node, list):
        for v in node:
            collect_refs(v)

collect_refs(root)
while queue:
    rid = queue.pop()
    if rid in seen:
        continue
    seen.add(rid)
    try:
        obj = get(rid)
    except subprocess.CalledProcessError:
        continue
    walk(obj, refs)
    collect_refs(obj)

if not refs:
    sys.exit("no attachments found in the result bundle")

# Only our own captures. A failing run also attaches XCUITest's debug material — UI hierarchy
# dumps, synthesized events, a screen recording — none of which are PNGs despite the name, and
# all of which land in the deliverable directory and break the next step's size probe.
import re
written = 0
for name, ref in sorted(set(refs)):
    if not re.match(r"^\d\d-", name):
        continue
    dest = out / f"{name}.png"
    with open(dest, "wb") as fh:
        subprocess.check_call(
            ["xcrun", "xcresulttool", "export", "object", "--legacy", "--path", bundle,
             "--id", ref, "--type", "file", "--output-path", "/dev/stdout"],
            stdout=fh, stderr=subprocess.DEVNULL)
    written += 1
    print(f"  {dest.name}")
print(f"{written} capture(s)")
PY

# The verdict comes after the extraction, and it is about the captures rather than the exit code:
# a late assertion failure that still produced every panel is a slow node, not a broken harness.
missing=()
for want in 01-empty 02-invite 03-scan 04-fleet 05-dashboard; do
  [[ -f "$out/$want.png" ]] || missing+=("$want")
done
if [[ ${#missing[@]} -gt 0 ]]; then
  die "missing captures: ${missing[*]} (see $here/.build/shots-$DEVICE.log)"
fi
[[ $status -eq 0 ]] || log "the harness reported a failure but every capture is present — see the log"

log "raw captures in $out"
for f in "$out"/*.png; do
  [[ -e "$f" ]] || continue
  printf '    %s  %s\n' "$(basename "$f")" \
    "$(sips -g pixelWidth -g pixelHeight "$f" 2>/dev/null | awk '/pixel/{printf "%s ", $2}')"
done
