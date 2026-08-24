#!/usr/bin/env bash
# The whole spike, end to end: a real adi node, a browser tab that is its own iroh peer, and a
# verdict written to report.json.
#
# Nothing here touches the machine's live state. The node runs against a *scratch store*
# ($HOME/.adi-mesh-spike/mono, via ADI_DIR) with its own identity, its own fleet registry and its
# own route table, and its gateway listener is moved off the platform default because the live
# control panel already holds that port. The operator's own ~/.adi/mono is only ever read.
#
#   ./run.sh            build what is missing, run the node, run the tab, print the verdict
#   ./run.sh --keep     leave the node and the page server up afterwards, for poking by hand
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$HERE/../.." && pwd)"

# The scratch store. Exported *here* and nowhere else: every adi tool resolves its state through
# $HOME/$ADI_DIR/mono, so leaking this into an interactive shell would silently point the CLI at
# an empty store.
export ADI_DIR=".adi-mesh-spike"
STORE="$HOME/$ADI_DIR/mono"

# The relay the *node* calls home, and therefore the one every byte of a browser session crosses.
# `NODE_RELAY=https://… ./run.sh` points the scratch node somewhere else — which is how the
# question "can a tab reach a node left on the shipped default?" gets an answer.
RELAY="${NODE_RELAY:-https://mad.mono-relay.withadi.dev}"
N0_RELAY="https://euw1-1.relay.iroh.network"   # a relay the fleet does not use, for case 5
SERVICE="spike"
SERVICE_PORT=45081       # the node's "local service", reached over the mesh
PAGE_PORT=45080          # the spike page + the report collector
GATEWAY_ADDR="127.0.0.1:45082"   # not the platform default: adi-app holds 10080 on this machine
PASSWORD="spike-password"
USERNAME="adi"
CHROME="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"

pids=()
cleanup() {
  [[ "${KEEP:-0}" == "1" ]] && { echo "left running: ${pids[*]}"; return; }
  for pid in "${pids[@]:-}"; do [[ -n "$pid" ]] && kill "$pid" 2>/dev/null || true; done
}
trap cleanup EXIT
[[ "${1:-}" == "--keep" ]] && KEEP=1

say() { printf '\n\033[1m== %s\033[0m\n' "$*"; }

# --- 1. the two builds ---------------------------------------------------------------------
say "building the node binary and the wasm bundle"
(cd "$ROOT" && cargo build -p adi-mesh 2>&1 | tail -3)
# `PROFILE=release ./run.sh` builds the bundle the way it would be shipped — wasm-opt run, debug
# info gone, a quarter the bytes. The dev profile is the default only because it builds faster.
(cd "$HERE" && wasm-pack build --target web "--${PROFILE:-dev}" --out-dir pkg 2>&1 | tail -2)

# --- 2. the scratch store ------------------------------------------------------------------
say "writing the scratch node's store at $STORE"
mkdir -p "$STORE/mesh" "$STORE/hive" "$HERE/service"

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

cat > "$HERE/service/index.html" <<EOF
<!doctype html><meta charset=utf-8><title>node-local service</title>
<h1>Hello from the node's local service on 127.0.0.1:$SERVICE_PORT.</h1>
<p>If a browser tab rendered this over the mesh, it dialled an iroh endpoint itself.</p>
EOF

# --- 3. the browser's identity -------------------------------------------------------------
#
# Minted out here rather than in the tab, because a node authorizes a peer *by key* and the key
# therefore has to be in its registry before the tab dials. ed25519 via openssl: a PKCS#8 v1
# private key ends with the 32-byte seed, and the SubjectPublicKeyInfo ends with the 32-byte
# public key — which is exactly iroh's `SecretKey::from_bytes` input and its `EndpointId`.
say "minting the browser's iroh identity"
openssl genpkey -algorithm ED25519 -out "$HERE/browser.pem" 2>/dev/null
BROWSER_SECRET=$(openssl pkey -in "$HERE/browser.pem" -outform DER | tail -c 32 | xxd -p -c 32)
BROWSER_KEY=$(openssl pkey -in "$HERE/browser.pem" -pubout -outform DER | tail -c 32 | xxd -p -c 32)
echo "browser key: $BROWSER_KEY"

# --- 4. pair it, by hand ---------------------------------------------------------------------
#
# What `adi-mono mesh join` would write, written directly: the browser cannot spend an invite yet
# (that is ADI-13's work, not the transport's). The grant is one service and nothing else, so the
# spike also proves the node's default-deny actually admits on the grant it was given.
cat > "$STORE/mesh/fleet.toml" <<EOF
[nodes.browser]
key = "$BROWSER_KEY"
nickname = "browser"
paired_at = $(date +%s)
grants = ["http:$SERVICE"]

[nodes.browser.auth]
EOF
adi-mono mesh passwd browser --password "$PASSWORD" --username "$USERNAME" >/dev/null

# --- 5. the node's own local service ---------------------------------------------------------
say "starting the node's local service on 127.0.0.1:$SERVICE_PORT"
(cd "$HERE/service" && exec python3 -m http.server "$SERVICE_PORT" --bind 127.0.0.1) \
  >"$HERE/service.log" 2>&1 &
pids+=($!)

# --- 6. the node ------------------------------------------------------------------------------
say "starting the scratch adi-mesh node"
ADI_MESH_GATEWAY_ADDR="$GATEWAY_ADDR" RUST_LOG="${RUST_LOG:-info}" \
  "$ROOT/target/debug/adi-mesh" run >"$HERE/node.log" 2>&1 &
pids+=($!)

NODE_KEY=$("$ROOT/target/debug/adi-mesh" id)
echo "node key:    $NODE_KEY"
# The node needs its relay session before anything can reach it through one.
for _ in $(seq 1 40); do grep -q "endpoint bound" "$HERE/node.log" && break; sleep 0.5; done
sleep 3

# --- 7. the page ------------------------------------------------------------------------------
say "serving the spike page on 127.0.0.1:$PAGE_PORT"
python3 "$HERE/serve.py" "$PAGE_PORT" >"$HERE/serve.log" 2>&1 &
pids+=($!)
sleep 1

URL="http://127.0.0.1:$PAGE_PORT/?secret=$BROWSER_SECRET&node=$NODE_KEY&relay=$RELAY&service=$SERVICE&user=$USERNAME&pass=$PASSWORD&path=/"
echo "$URL" > "$HERE/spike-url.txt"

# A second identity that nothing ever paired, for the case below.
openssl genpkey -algorithm ED25519 -out "$HERE/stranger.pem" 2>/dev/null
STRANGER_SECRET=$(openssl pkey -in "$HERE/stranger.pem" -outform DER | tail -c 32 | xxd -p -c 32)

# --- 8. the tab, once per case ----------------------------------------------------------------
#
# The happy path alone would not tell you whether the node's gate is in the path at all — a stub
# that answered `Ok` to everything would look identical. Each case below changes exactly one thing
# about the same tab, and the node has to refuse it for its own stated reason.
rm -rf "$HERE"/.chrome-profile-*
case_n=0
probe() {
  local label="$1" query="$2" chrome_pid
  rm -f "$HERE/report.json"
  # A profile directory of its own per case, and the browser is stopped again afterwards: Chrome
  # refuses a second `--user-data-dir` on a profile already open and silently hands the URL to the
  # running instance instead — which, headless, means the page is never loaded and the case looks
  # like a hang rather than a re-use.
  case_n=$((case_n + 1))
  # `--enable-logging=stderr` is what puts the page's console — and so iroh's own tracing, which
  # `wasm-tracing` routes there — into chrome.log. It is the only view of the relay handshake this
  # transport has, and without it a failed dial is one error string.
  "$CHROME" --headless=new --disable-gpu --no-first-run --no-default-browser-check \
    --enable-logging=stderr --v=0 \
    --user-data-dir="$HERE/.chrome-profile-$case_n" \
    "http://127.0.0.1:$PAGE_PORT/?$query" >>"$HERE/chrome-$case_n.log" 2>&1 &
  chrome_pid=$!
  for _ in $(seq 1 120); do [[ -f "$HERE/report.json" ]] && break; sleep 1; done
  kill "$chrome_pid" 2>/dev/null || true
  if [[ ! -f "$HERE/report.json" ]]; then
    echo "FAILED to report: $label — see chrome-*.log, serve.log, node.log"
    return 1
  fi
  printf '\n\033[1m--- %s\033[0m\n' "$label"
  cat "$HERE/report.json"
  cp "$HERE/report.json" "$HERE/report-$(echo "$label" | tr ' ' '-').json"
}

say "opening the page in headless Chrome"
probe "1 granted service, right password" \
  "secret=$BROWSER_SECRET&node=$NODE_KEY&relay=$RELAY&service=$SERVICE&user=$USERNAME&pass=$PASSWORD&path=/"
probe "2 granted service, wrong password" \
  "secret=$BROWSER_SECRET&node=$NODE_KEY&relay=$RELAY&service=$SERVICE&user=$USERNAME&pass=nope&path=/"
probe "3 a service this browser holds no grant for" \
  "secret=$BROWSER_SECRET&node=$NODE_KEY&relay=$RELAY&service=app&user=$USERNAME&pass=$PASSWORD&path=/"
probe "4 a browser key the node never paired" \
  "secret=$STRANGER_SECRET&node=$NODE_KEY&relay=$RELAY&service=$SERVICE&user=$USERNAME&pass=$PASSWORD&path=/"
# Does the tab have to share the node's relay, or is knowing the node's enough? The answer decides
# whether a client served to everybody from one domain has to be told a relay per node.
probe "5 the tab's own relay is n0's, the node's is ours" \
  "secret=$BROWSER_SECRET&node=$NODE_KEY&relay=$RELAY&home_relay=$N0_RELAY&service=$SERVICE&user=$USERNAME&pass=$PASSWORD&path=/"
