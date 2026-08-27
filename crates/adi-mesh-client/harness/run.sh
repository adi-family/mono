#!/usr/bin/env bash
# Does a long-lived stream cross `adi/mesh/http/1` from a browser tab? — end to end.
#
# A real `adi-mesh` daemon, a real relay, and a real headless Chrome tab holding its own iroh key.
# The verdict lands in `report.json`; `src/probe.rs` says what each case proves.
#
# Nothing here touches the machine's live state. The node runs against a *scratch store*
# ($HOME/.adi-mesh-spike/mono, via ADI_DIR) with its own identity, fleet registry and route table,
# and its gateway listener is moved off the platform default because the live control panel holds
# that port. The operator's own ~/.adi/mono is only ever read.
#
#   ./run.sh              build what is missing, run the node, run the probe, print the verdict
#   ./run.sh --keep       leave the node and the page server up afterwards, for poking by hand
#   PROFILE=release       build the bundle the way it ships (wasm-opt run, a quarter the bytes)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CRATE="$(cd "$HERE/.." && pwd)"
ROOT="$(cd "$CRATE/../.." && pwd)"

# The scratch store. Exported *here* and nowhere else: every adi tool resolves its state through
# $HOME/$ADI_DIR/mono, so leaking this into an interactive shell would silently point the CLI at
# an empty store.
export ADI_DIR=".adi-mesh-spike"
STORE="$HOME/$ADI_DIR/mono"

RELAY="${NODE_RELAY:-https://mad.mono-relay.withadi.dev}"
SERVICE="probe"
SERVICE_PORT=45081        # the node's "local service", reached over the mesh
PAGE_PORT=45080           # the probe page + the report collector
GATEWAY_ADDR="127.0.0.1:45082"   # not the platform default: adi-app holds 10080 on this machine
PASSWORD="probe-password"
USERNAME="adi"
EVENTS="${EVENTS:-5}"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

pids=()
cleanup() {
  [[ "${KEEP:-0}" == "1" ]] && { echo "left running: ${pids[*]}"; return; }
  for pid in "${pids[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT
[[ "${1:-}" == "--keep" ]] && KEEP=1

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

# --- 1. the two builds -----------------------------------------------------------------------
say "building the node binary and the wasm bundle"
(cd "$ROOT" && cargo build -p adi-mesh 2>&1 | tail -3)
# From the crate directory, so `.cargo/config.toml` there is on cargo's search path — without it
# `ring` cannot compile C for wasm on a Mac. This is why there is no `-p adi-mesh-client` here.
(cd "$CRATE" && wasm-pack build --target web "--${PROFILE:-dev}" --out-dir "$HERE/pkg" 2>&1 | tail -2)

# --- 2. the scratch store --------------------------------------------------------------------
say "writing the scratch node's store at $STORE"
mkdir -p "$STORE/mesh" "$STORE/hive"

cat > "$STORE/mesh/mesh.toml" <<EOF
# The fleet's own relay, not n0's — a browser has no UDP, so this is the whole data path.
relays = ["$RELAY"]
forwards = []

[host]
# Nothing is exposed over the raw forward ALPN: this node serves the HTTP gateway only.
allow = []
authorized_peers = []
EOF

cat > "$STORE/hive/hive.yaml" <<EOF
# The node's route table. The gateway resolves a service label off the wire as <label>.adi
# against exactly this file (adi-mesh/src/gateway.rs, \`Routes::resolve\`).
version: "1"
services:
  $SERVICE:
    proxy:
      host: $SERVICE.adi
    rollout:
      recreate:
        ports:
          http: $SERVICE_PORT
EOF

# --- 3. the browser's identity ----------------------------------------------------------------
#
# Minted out here rather than in the tab, because a node authorizes a peer *by key* and the key
# therefore has to be in its registry before the tab dials. ed25519 via openssl: a PKCS#8 v1
# private key ends with the 32-byte seed, and the SubjectPublicKeyInfo ends with the 32-byte
# public key — which is exactly iroh's `SecretKey::from_bytes` input and its `EndpointId`.
#
# (Pairing proper is `adi-mono mesh invite` on the node plus the client's Add-a-node screen; this
# probe is about the transport, so it writes the registry entry directly and skips the handshake.)
say "minting the browser's iroh identity"
openssl genpkey -algorithm ED25519 -out "$HERE/browser.pem" 2>/dev/null
BROWSER_SECRET=$(openssl pkey -in "$HERE/browser.pem" -outform DER | tail -c 32 | xxd -p -c 32)
BROWSER_KEY=$(openssl pkey -in "$HERE/browser.pem" -pubout -outform DER | tail -c 32 | xxd -p -c 32)
echo "browser key: $BROWSER_KEY"

cat > "$STORE/mesh/fleet.toml" <<EOF
[nodes.browser]
key = "$BROWSER_KEY"
nickname = "browser"
paired_at = $(date +%s)
grants = ["http:$SERVICE"]

[nodes.browser.auth]
EOF
adi-mono mesh passwd browser --password "$PASSWORD" --username "$USERNAME" >/dev/null

# --- 4. the node's local service ---------------------------------------------------------------
say "starting the node's local service on 127.0.0.1:$SERVICE_PORT"
python3 "$HERE/upstream.py" "$SERVICE_PORT" >"$HERE/upstream.log" 2>&1 &
pids+=($!)

# --- 5. the node --------------------------------------------------------------------------------
say "starting the scratch adi-mesh node"
ADI_MESH_GATEWAY_ADDR="$GATEWAY_ADDR" RUST_LOG="${RUST_LOG:-info}" \
  "$ROOT/target/debug/adi-mesh" run >"$HERE/node.log" 2>&1 &
pids+=($!)

NODE_KEY=$("$ROOT/target/debug/adi-mesh" id)
echo "node key:    $NODE_KEY"
# The node needs its relay session before anything can reach it through one.
for _ in $(seq 1 40); do grep -q "endpoint bound" "$HERE/node.log" && break; sleep 0.5; done
sleep 3

# --- 6. the page ---------------------------------------------------------------------------------
say "serving the probe page on 127.0.0.1:$PAGE_PORT"
python3 "$HERE/serve.py" "$PAGE_PORT" >"$HERE/serve.log" 2>&1 &
pids+=($!)
sleep 1

QUERY="secret=$BROWSER_SECRET&node=$NODE_KEY&relay=$RELAY&service=$SERVICE&user=$USERNAME&pass=$PASSWORD&events=$EVENTS"
echo "http://127.0.0.1:$PAGE_PORT/?$QUERY" > "$HERE/probe-url.txt"

# --- 7. the tab ------------------------------------------------------------------------------------
say "opening the page in headless Chrome"
rm -rf "$HERE"/.chrome-profile "$HERE/report.json"
# A profile directory of its own, and the browser is stopped again afterwards: Chrome refuses a
# second `--user-data-dir` on a profile already open and silently hands the URL to the running
# instance instead — which, headless, means the page is never loaded and the run looks like a hang.
#
# `--enable-logging=stderr` is what puts the page's console — and so iroh's own tracing — into
# chrome.log. It is the only view of the relay handshake this transport has.
"$CHROME" --headless=new --disable-gpu --no-first-run --no-default-browser-check \
  --enable-logging=stderr --v=0 --user-data-dir="$HERE/.chrome-profile" \
  "http://127.0.0.1:$PAGE_PORT/?$QUERY" >"$HERE/chrome.log" 2>&1 &
chrome_pid=$!
for _ in $(seq 1 180); do [[ -f "$HERE/report.json" ]] && break; sleep 1; done
kill "$chrome_pid" 2>/dev/null || true

if [[ ! -f "$HERE/report.json" ]]; then
  echo "FAILED to report — see chrome.log, serve.log, node.log, upstream.log"
  exit 1
fi

say "verdict"
cat "$HERE/report.json"
python3 - "$HERE/report.json" <<'PY'
import json, sys
report = json.load(open(sys.argv[1]))
print()
for case in report.get("cases", []):
    print(f"{'PASS' if case['ok'] else 'FAIL'}  {case['name']}: {case['detail'] or case['error']}")
print(f"\n{'ALL CASES PASSED' if report.get('ok') else 'SOMETHING FAILED'}")
sys.exit(0 if report.get("ok") else 1)
PY
