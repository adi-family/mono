#!/usr/bin/env bash
# The whole client, end to end: pair with a real node using a real invite, and open its panel.
#
# What this is actually checking, in order — each one is a thing that could only be found by
# running it in a browser:
#
#   1. the browser-initiated join: the tab spends an `adi-invite:` the *node* minted, and the node
#      files this tab's key with `http:app` and a password (`docs/fleet.md` §8)
#   2. the service worker answering a whole app from a node, by client id
#   3. the panel shim giving that app back `/` as its own path
#   4. a root-absolute `/api/*` from inside the panel reaching the same node
#   5. `new WebSocket()` inside the panel crossing `adi/mesh/http/1`
#
# Same scratch store and the same rules as `run.sh`: nothing here touches `~/.adi/mono`, and the
# node's gateway listener is moved off the platform default because the live control panel holds it.
#
#   ./e2e.sh              build, run, verify, tear down
#   ./e2e.sh --keep       leave everything up afterwards, for poking by hand
#   SKIP_BUILD=1          reuse the dist/ that is already there
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE="$(cd "$HERE/.." && pwd)"
ROOT="$(cd "$CRATE/../.." && pwd)"

export ADI_DIR=".adi-mesh-spike"
STORE="$HOME/$ADI_DIR/mono"

RELAY="${NODE_RELAY:-https://mad.mono-relay.withadi.dev}"
SERVICE_PORT=45081
PAGE_PORT=45080
GATEWAY_ADDR="127.0.0.1:45082"

pids=()
cleanup() {
  rm -f "$CRATE/dist/e2e.html"
  [[ "${KEEP:-0}" == "1" ]] && { echo "left running: ${pids[*]}"; return; }
  for pid in "${pids[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT
[[ "${1:-}" == "--keep" ]] && KEEP=1

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

# --- 1. builds ---------------------------------------------------------------------------------
if [[ "${SKIP_BUILD:-0}" != "1" ]]; then
  say "building the node binary and the client"
  (cd "$ROOT" && cargo build -p adi-mesh 2>&1 | tail -3)
  # From the crate directory: `.cargo/config.toml` there is what lets `ring` compile C for wasm.
  (cd "$CRATE" && trunk build --release 2>&1 | tail -3)
fi
[[ -f "$CRATE/dist/index.html" ]] || { echo "no dist/ — run without SKIP_BUILD"; exit 1; }
# Served from `dist/` so the driver is same-origin with the client and the worker controls it.
cp "$HERE/e2e.html" "$CRATE/dist/e2e.html"

# --- 2. the scratch node -----------------------------------------------------------------------
say "writing the scratch node's store at $STORE"
mkdir -p "$STORE/mesh" "$STORE/hive"

cat > "$STORE/mesh/mesh.toml" <<EOF
relays = ["$RELAY"]
forwards = []

[host]
allow = []
authorized_peers = []
EOF

# `app` is the service a pairing grants (`docs/fleet.md` §8) and the only one this client opens.
cat > "$STORE/hive/hive.yaml" <<EOF
version: "1"
services:
  app:
    proxy:
      host: app.adi
    rollout:
      recreate:
        ports:
          http: ${APP_PORT:-$SERVICE_PORT}
  probe:
    proxy:
      host: probe.adi
    rollout:
      recreate:
        ports:
          http: $SERVICE_PORT
EOF

# An empty registry: the browser's key gets in through the join handshake and nothing else.
# `invites.toml` is cleared too, so a nonce left over from a previous run cannot be the one spent.
printf '' > "$STORE/mesh/fleet.toml"
rm -f "$STORE/mesh/invites.toml"

if [[ -z "${APP_PORT:-}" ]]; then
  say "starting the node's local service on 127.0.0.1:$SERVICE_PORT"
  python3 "$HERE/upstream.py" "$SERVICE_PORT" >"$HERE/upstream.log" 2>&1 &
  pids+=($!)
else
  say "the node's \`app\` service points at 127.0.0.1:$APP_PORT (not started by this script)"
fi

say "starting the scratch adi-mesh node"
ADI_MESH_GATEWAY_ADDR="$GATEWAY_ADDR" RUST_LOG="${RUST_LOG:-info}" \
  "$ROOT/target/debug/adi-mesh" run >"$HERE/node.log" 2>&1 &
pids+=($!)
for _ in $(seq 1 40); do grep -q "endpoint bound" "$HERE/node.log" && break; sleep 0.5; done
# The invite carries the node's published ticket, and the daemon only publishes it once its relay
# session is up — mint too early and the token names an endpoint with no relay in it.
for _ in $(seq 1 40); do [[ -s "$STORE/mesh/ticket" ]] && break; sleep 0.5; done
echo "node key:    $("$ROOT/target/debug/adi-mesh" id)"

# --- 3. the invite, minted on the node ----------------------------------------------------------
#
# This is the direction the whole design turns on. `mesh invite` is normally run on the machine you
# are *sitting at*, and the headless node dials it. Here it is run on the node, because nothing can
# dial a browser — and the handshake does not care which of them is which.
say "minting an invite on the node"
INVITE=$(adi-mono mesh invite --ttl 15 2>/dev/null | tr -d '[:space:]' | grep -o 'adi-invite:[0-9a-f]*' | head -1)
[[ -n "$INVITE" ]] || { echo "could not mint an invite — see $HERE/node.log"; exit 1; }
echo "invite:      ${INVITE:0:40}…"

# --- 4. the page --------------------------------------------------------------------------------
say "serving dist/ on 127.0.0.1:$PAGE_PORT"
python3 "$HERE/serve.py" "$PAGE_PORT" "$CRATE/dist" >"$HERE/serve.log" 2>&1 &
pids+=($!)
sleep 1

URL="http://127.0.0.1:$PAGE_PORT/e2e.html?invite=$INVITE&real=${REAL:-0}"
echo "$URL" > "$HERE/e2e-url.txt"

# --- 5. the browser ------------------------------------------------------------------------------
say "opening the client in headless Chrome"
CHROME="${CHROME:-/Applications/Google Chrome.app/Contents/MacOS/Google Chrome}"
rm -rf "$HERE"/.chrome-e2e "$HERE/report.json"
# 127.0.0.1 is a *secure context*, so service workers, IndexedDB and the install prompt all behave
# exactly as they would over HTTPS. That is what makes this a real test of the deployed thing.
"$CHROME" --headless=new --disable-gpu --no-first-run --no-default-browser-check \
  --enable-logging=stderr --v=0 --user-data-dir="$HERE/.chrome-e2e" \
  "$URL" >"$HERE/chrome-e2e.log" 2>&1 &
chrome_pid=$!
for _ in $(seq 1 240); do [[ -f "$HERE/report.json" ]] && break; sleep 1; done
kill "$chrome_pid" 2>/dev/null || true

if [[ ! -f "$HERE/report.json" ]]; then
  echo "FAILED to report — see chrome-e2e.log, serve.log, node.log, upstream.log"
  exit 1
fi

say "verdict"
cat "$HERE/report.json"
python3 - "$HERE/report.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
print()
for name, value in (report.get("findings") or {}).items():
    print(f"  {name}: {value}")
print(f"\n{'END TO END PASSED' if report.get('ok') else 'FAILED: ' + str(report.get('error'))}")
sys.exit(0 if report.get("ok") else 1)
PY
