#!/usr/bin/env bash
# Bring up one more `iroh-relay` region for the fleet, on GCE.
#
# A relay is the fleet's fallback data path, not a signalling service (`docs/fleet.md` §9): while a
# direct path exists it carries nothing, and while one does not it carries *everything*. iroh probes
# every relay in a machine's map and settles that machine on its own **nearest** one, so a second
# region is not a migration of anything — it is one more entry, and the machines near it move over on
# their own. Nothing is addressed by relay name, so no peer, invite or pairing is re-issued.
#
#   scripts/deploy-relay.sh iad us-east4-a
#   scripts/deploy-relay.sh iad us-east4-a --dry-run       # print every command, run none
#
# The label is the short region name that becomes the hostname's first label —
# `<label>.mono-relay.withadi.dev`, matching `mad` for Madrid. Use the IATA code of the city the zone
# is in, so the name says where the packets go. Everything it creates is named `adi-relay-<label>`
# and carries the tag `adi-relay`, which is what `adi-relay-ingress` opens.
#
# This is `adi-relay-mad` written down. It was built by hand on 2026-08-05 and read back off the
# running box on 2026-08-27 — same image, same machine type, same unit, same config layout — so a
# second region is not a second design.
#
# ## The one step this cannot do for you
#
# **The DNS A record.** LetsEncrypt's http-01 challenge is answered by the relay itself on :80, so
# the hostname has to resolve to the VM's address *before* the relay first starts, or it comes up
# with no certificate and every client fails the TLS handshake. The zone `withadi.dev` is on
# Cloudflare and the token this machine holds carries `zone (read)` only
# (`crates/adi-mesh-client/DEPLOY.md` hit the same wall), so the record is added by a human — or by
# this script, if a token with *Zone → DNS → Edit* is stored as `CLOUDFLARE_DNS_TOKEN`.
#
# So the order is: reserve the address, publish it, then create the VM. This script does exactly
# that, and stops in the middle with the address printed if it cannot publish it itself. Every step
# is idempotent, so re-running it once the record is live picks up where it stopped.
#
# ## What ends up on the VM
#
# Debian 12, no Docker, no build toolchain: the startup script downloads a released `iroh-relay` as a
# static musl binary, writes `/etc/iroh-relay/relay.toml` and a systemd unit that runs it as an
# unprivileged system user with `CAP_NET_BIND_SERVICE` and nothing else. It listens on :80 (ACME),
# :443 (the protocol) and **UDP 7842** (QUIC address discovery) — that last one is what lets a direct
# path form at all, so a relay behind anything that drops UDP is worse than no relay.
#
# The relay is deliberately **open** — new installs pair without configuration — and therefore
# **rate-limited**, in the `[limits.client.rx]` block of the config below, which is where that
# decision is written down. Note that the startup script provisions **once**: it exits early when
# the binary and the config are already there, so a relay that is already running does not pick
# these limits up on a reboot. Applying them to `adi-relay-mad` means editing
# `/etc/iroh-relay/relay.toml` on that box and `systemctl restart iroh-relay` — a live change, and
# so the operator's call, not this script's.
#
# ## Why the relay is pinned to 1.0.1 while the workspace is on iroh 1.0.3
#
# The obvious default — track `Cargo.lock` — was tried first, on this box, and 1.0.3 **aborted the
# whole process** within a minute of its first traffic:
#
#     noq-udp-1.1.0/src/cmsg/mod.rs:81: assertion failed: align_of::<T>() <= align_of::<C>()
#
# That assert is in `decode`, the path that reads an inbound control message, so it is reachable from
# received UDP — on the exact half of the relay that QUIC address discovery is for. 1.0.3's changelog
# names "update to noq 1.1.0" as its own change, and the assert is still there in noq-udp 1.1.1.
# Meanwhile `adi-relay-mad` has run 1.0.1 since 2026-08-05 with `NRestarts=0` and not one panic.
#
# The relay does not have to match the client: the fleet's nodes are on iroh 1.0.3 and have talked to
# a 1.0.1 relay every day since. Both speak `iroh-relay-v2`, which is the compatibility surface that
# actually exists. So the fleet runs one proven relay build; raise this only with a reason.
set -euo pipefail

# **Not** the workspace's iroh version, and that is deliberate — see the note below.
IROH_VERSION="v1.0.1"
PROJECT="mono-504617"
DOMAIN="mono-relay.withadi.dev"
CONTACT="ihor@withadi.dev"     # LetsEncrypt refuses to start without one
MACHINE_TYPE="e2-micro"        # what `mad` runs; the relay forwards bytes, it does not compute
DISK_GB=10
ACCOUNT=""
DRY_RUN=0
SKIP_DNS_WAIT=0

die() { echo "error: $*" >&2; exit 1; }
usage() {
  # The header block, however long it has grown: everything down to the first line of code.
  awk 'NR>1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  echo
  echo "Options: --project P  --domain D  --contact E  --machine-type T  --account A"
  echo "         --iroh-version V  --dry-run  --skip-dns-wait"
  exit "${1:-0}"
}

[[ $# -ge 2 ]] || usage 1
LABEL="$1"; ZONE="$2"; shift 2
while [[ $# -gt 0 ]]; do
  case "$1" in
    --project) PROJECT="$2"; shift 2 ;;
    --domain) DOMAIN="$2"; shift 2 ;;
    --contact) CONTACT="$2"; shift 2 ;;
    --machine-type) MACHINE_TYPE="$2"; shift 2 ;;
    --account) ACCOUNT="$2"; shift 2 ;;
    --iroh-version) IROH_VERSION="$2"; shift 2 ;;
    --dry-run) DRY_RUN=1; shift ;;
    --skip-dns-wait) SKIP_DNS_WAIT=1; shift ;;
    -h|--help) usage 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ "$LABEL" =~ ^[a-z0-9][a-z0-9-]{0,20}$ ]] || die "label must be one lowercase DNS label: $LABEL"
[[ "$ZONE" =~ ^[a-z0-9-]+-[a-z]$ ]] || die "zone looks wrong (want e.g. us-east4-a): $ZONE"

REGION="${ZONE%-*}"
FQDN="$LABEL.$DOMAIN"
NAME="adi-relay-$LABEL"        # the VM and its address share a name, as `mad` does
TAG="adi-relay"
FW="adi-relay-ingress"

command -v gcloud >/dev/null || die "gcloud is not installed"

gc=(gcloud --project="$PROJECT" --quiet)
[[ -n "$ACCOUNT" ]] && gc+=(--account="$ACCOUNT")

run() {
  if [[ $DRY_RUN -eq 1 ]]; then printf '  + %q' "$1"; printf ' %q' "${@:2}"; printf '\n'; return 0; fi
  "$@"
}

echo "==> relay $FQDN  →  project $PROJECT, zone $ZONE, $MACHINE_TYPE, iroh-relay $IROH_VERSION"

if [[ $DRY_RUN -eq 0 ]]; then
  "${gc[@]}" projects describe "$PROJECT" >/dev/null 2>&1 \
    || die "cannot reach project $PROJECT. Run:  gcloud auth login --account=$CONTACT"
fi

# ---------------------------------------------------------------------------- 1. the address
# Static and regional. An ephemeral address survives a reboot but not a stop/start, and the whole
# point of the DNS record is that it keeps pointing at the relay.
echo "==> static address $NAME in $REGION"
if "${gc[@]}" compute addresses describe "$NAME" --region="$REGION" >/dev/null 2>&1; then
  echo "    already reserved"
else
  run "${gc[@]}" compute addresses create "$NAME" --region="$REGION" \
    --description="iroh-relay $REGION — $FQDN"
fi
IP="$("${gc[@]}" compute addresses describe "$NAME" --region="$REGION" \
      --format='value(address)' 2>/dev/null || true)"
[[ -n "$IP" || $DRY_RUN -eq 1 ]] || die "could not read back the reserved address"
[[ -z "$IP" ]] && IP="<reserved-ip>"
echo "    address: $IP"

# ---------------------------------------------------------------------------- 2. the firewall
# One rule for the whole fleet, on the tag, so a second region reuses it rather than adding its own.
echo "==> firewall rule $FW on tag $TAG"
if "${gc[@]}" compute firewall-rules describe "$FW" >/dev/null 2>&1; then
  echo "    exists — every tagged relay is covered"
else
  run "${gc[@]}" compute firewall-rules create "$FW" \
    --allow=tcp:80,tcp:443,udp:7842 --target-tags="$TAG" --source-ranges=0.0.0.0/0 \
    --description="iroh-relay: ACME on :80, the protocol on :443, QUIC address discovery on udp:7842"
fi

# ---------------------------------------------------------------------------- 3. the DNS record
echo "==> DNS: $FQDN must resolve to $IP before the relay first starts"
publish_via_cloudflare() {
  local token zone_id zone_name
  token="$(adi-mono secrets read CLOUDFLARE_DNS_TOKEN 2>/dev/null || true)"
  [[ -n "$token" ]] || return 1
  zone_name="${DOMAIN#*.}"          # mono-relay.withadi.dev → withadi.dev
  zone_id="$(curl -fsS -H "Authorization: Bearer $token" \
    "https://api.cloudflare.com/client/v4/zones?name=$zone_name" \
    | python3 -c 'import json,sys; r=json.load(sys.stdin)["result"]; print(r[0]["id"] if r else "")')"
  [[ -n "$zone_id" ]] || return 1
  # Unproxied: the relay terminates its own TLS and speaks QUIC over UDP, neither of which survives
  # Cloudflare's proxy. `mad` is a grey-cloud A record for the same reason.
  curl -fsS -X POST -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
    "https://api.cloudflare.com/client/v4/zones/$zone_id/dns_records" \
    -d "{\"type\":\"A\",\"name\":\"$FQDN\",\"content\":\"$IP\",\"proxied\":false,\"ttl\":300}" \
    >/dev/null
}
if [[ $DRY_RUN -eq 1 ]]; then
  echo "    (dry run — would publish A $FQDN → $IP, unproxied)"
elif [[ "$(dig +short A "$FQDN" | tail -1)" == "$IP" ]]; then
  echo "    already resolves"
elif publish_via_cloudflare; then
  echo "    published via the Cloudflare API"
else
  cat <<EOF

    This machine cannot write the record (no CLOUDFLARE_DNS_TOKEN, or the API refused).
    Add it by hand, then re-run this script — everything above is idempotent:

      withadi.dev → DNS → add record
        type    A
        name    $LABEL.${DOMAIN%%.*}
        content $IP
        proxy   DNS only (grey cloud)   ← the relay does its own TLS and needs raw UDP
        TTL     auto

    To make this unattended next time, mint a token with Zone → DNS → Edit on withadi.dev and
    store it:  adi-mono secrets set CLOUDFLARE_DNS_TOKEN
EOF
  exit 2
fi

if [[ $DRY_RUN -eq 0 && $SKIP_DNS_WAIT -eq 0 ]]; then
  echo -n "    waiting for it to resolve here"
  for _ in $(seq 1 60); do
    [[ "$(dig +short A "$FQDN" | tail -1)" == "$IP" ]] && { echo " ok"; break; }
    echo -n "."; sleep 5
  done
  [[ "$(dig +short A "$FQDN" | tail -1)" == "$IP" ]] \
    || die "$FQDN still does not resolve to $IP; LetsEncrypt would fail. Re-run when it does."
fi

# ---------------------------------------------------------------------------- 4. the VM
# Everything below is what is on `adi-relay-mad`, with the hostname substituted.
startup="$(cat <<EOF
#!/usr/bin/env bash
set -euo pipefail
arch="\$(uname -m)"
case "\$arch" in x86_64) target=x86_64 ;; aarch64) target=aarch64 ;; *) echo "unsupported: \$arch" >&2; exit 1 ;; esac

# Already provisioned? A startup script runs on every boot, not just the first.
if [[ -x /usr/local/bin/iroh-relay && -f /etc/iroh-relay/relay.toml ]]; then exit 0; fi

url="https://github.com/n0-computer/iroh/releases/download/$IROH_VERSION/iroh-relay-$IROH_VERSION-\${target}-unknown-linux-musl.tar.gz"
tmp="\$(mktemp -d)"
curl -fsSL --retry 5 --retry-delay 3 "\$url" -o "\$tmp/relay.tgz"
tar -xzf "\$tmp/relay.tgz" -C "\$tmp"
install -m 0755 "\$(find "\$tmp" -type f -name iroh-relay | head -1)" /usr/local/bin/iroh-relay
rm -rf "\$tmp"

# An unprivileged system account. The unit gives it CAP_NET_BIND_SERVICE and nothing else.
id -u iroh-relay >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin iroh-relay
install -d -o iroh-relay -g iroh-relay -m 0755 /var/lib/iroh-relay /var/lib/iroh-relay/certs
install -d -m 0755 /etc/iroh-relay

cat >/etc/iroh-relay/relay.toml <<'CONF'
# iroh-relay for the adi fleet — docs/fleet.md §9, §10.
#
# Stateless by construction: the server keeps an in-memory map of connected peers and forwards
# encrypted datagrams between them. No datastore, nothing on disk but the ACME certificate.
# Restarting it costs a reconnect, never data.
#
# It is also UNTRUSTED by construction: peer identity is an Ed25519 key verified end to end, so
# this box can drop or delay traffic but can never read it or impersonate anyone on it.

enable_relay = true

# ACME http-01 lives here; the relay protocol itself is on 443 below.
http_bind_addr = "[::]:80"

# The half that earns the box: peers learn their own public address here, which is what makes a
# direct path (and therefore a relay that carries nothing) possible at all.
enable_quic_addr_discovery = true

[tls]
hostname = ["$FQDN"]
cert_mode = "LetsEncrypt"
contact = "$CONTACT"
prod_tls = true
cert_dir = "/var/lib/iroh-relay/certs"
https_bind_addr = "[::]:443"
quic_bind_addr = "[::]:7842"

# It is OPEN ON PURPOSE, and metered because of it. There is no access block below, so iroh's
# default stands and any client may connect. That is the product decision, not an oversight: a
# fresh install is meant to pair with no configuration at all (docs/fleet.md §9), and an
# allow-list of fleet keys would mean editing this file on every relay before a new machine
# could join — it would break the product, not just this fleet. What open must not also mean is
# unmetered, so a stranger who finds the hostname is capped here instead of excluded.
#
# Only one knob in iroh-relay 1.0.1 does anything. limits.accept_conn_limit and
# accept_conn_burst parse and are then ignored — server.rs carries the TODO saying they are not
# implemented — so they are deliberately left out: a limit that only exists in a config file
# reads like protection nobody has.
#
# limits.client.rx is real: a token bucket on each connected client's inbound stream
# (server/streams.rs, RateLimited). The size is what the relay is for — a panel read over the
# mesh when no direct path formed, which is one bundle and then JSON — so a burst covers a
# first page load and the steady rate holds a sustained abuser to a couple of TB a month per
# connection rather than a link's worth. While a direct path exists this carries nothing at all,
# so raising it only matters to machines that cannot hole-punch.
[limits.client.rx]
bytes_per_second = 1048576    # 1 MiB/s steady state
max_burst_bytes = 4194304     # 4 MiB, so the first load of a panel is not throttled
CONF

cat >/etc/systemd/system/iroh-relay.service <<'UNIT'
[Unit]
Description=iroh-relay — rendezvous and fallback path for the adi fleet
Documentation=https://github.com/n0-computer/iroh
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=iroh-relay
Group=iroh-relay
ExecStart=/usr/local/bin/iroh-relay --config-path /etc/iroh-relay/relay.toml
Restart=always
RestartSec=3
# 80 and 443 are privileged; this is the one capability it needs, instead of running as root.
AmbientCapabilities=CAP_NET_BIND_SERVICE
CapabilityBoundingSet=CAP_NET_BIND_SERVICE
StateDirectory=iroh-relay
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/iroh-relay

[Install]
WantedBy=multi-user.target
UNIT

systemctl daemon-reload
systemctl enable --now iroh-relay
EOF
)"

echo "==> creating $NAME"
if "${gc[@]}" compute instances describe "$NAME" --zone="$ZONE" >/dev/null 2>&1; then
  echo "    already exists — leaving it alone"
elif [[ $DRY_RUN -eq 1 ]]; then
  echo "  + gcloud compute instances create $NAME --zone=$ZONE --machine-type=$MACHINE_TYPE"
  echo "      --image-family=debian-12 --address=$IP --tags=$TAG --metadata-from-file=startup-script=..."
else
  startup_file="$(mktemp)"; printf '%s\n' "$startup" >"$startup_file"
  "${gc[@]}" compute instances create "$NAME" \
    --zone="$ZONE" \
    --machine-type="$MACHINE_TYPE" \
    --image-family=debian-12 --image-project=debian-cloud \
    --boot-disk-size="${DISK_GB}GB" \
    --address="$IP" \
    --tags="$TAG" \
    --metadata-from-file=startup-script="$startup_file" \
    --labels=role=iroh-relay
  rm -f "$startup_file"
fi

# ---------------------------------------------------------------------------- 5. verify
[[ $DRY_RUN -eq 1 ]] && { echo "==> dry run complete"; exit 0; }

echo "==> waiting for boot, install and the LetsEncrypt certificate"
for _ in $(seq 1 72); do
  curl -fsS -m 5 "https://$FQDN/" 2>/dev/null | grep -qi 'iroh relay' && break
  echo -n "."; sleep 5
done
echo

echo "--- https://$FQDN/"
curl -fsS -m 10 "https://$FQDN/" | head -3 || die "the relay is not answering over HTTPS"

# The check that actually matters for the browser client: RFC 6455 §4.1 makes a client fail a 101
# that does not echo the subprotocol it offered, and n0's public relay does not echo it — which is
# why the fleet runs its own at all (`crates/adi-mesh/src/relay.rs`). A relay that answers 200 on /
# but drops this header is useless to a tab.
#
# The upgrade SUCCEEDING is what makes curl exit non-zero: the socket stays open and streaming, so
# curl always ends on `28` (timed out). Hence the capture-then-match — piped into grep under
# `pipefail`, a working relay reads as a broken one.
echo "--- websocket upgrade"
# `-D - -o /dev/null` keeps the headers and drops the body — which on a successful upgrade is a
# binary relay frame, and would otherwise reach the shell as a null byte it warns about.
ws_response="$(curl -sS -m 5 -D - -o /dev/null \
      -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' \
      -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H 'Sec-WebSocket-Protocol: iroh-relay-v2' \
      "https://$FQDN/relay" 2>/dev/null || true)"
if grep -qi '^sec-websocket-protocol: iroh-relay-v2' <<<"$ws_response"; then
  echo "    101 echoes sec-websocket-protocol: iroh-relay-v2 — browser-compatible"
else
  die "the upgrade did not echo iroh-relay-v2; a browser will close this relay with code 1006"
fi

echo "--- UDP 7842 (QUIC address discovery)"
nc -z -u -w 5 "$FQDN" 7842 && echo "    reachable" || echo "    WARNING: no answer — direct paths will not form"

cat <<EOF

==> $FQDN is live at $IP

Next, and only now that it resolves:
  1. add "https://$FQDN" to DEFAULT_RELAYS in crates/adi-mesh/src/relay.rs
  2. name the region in docs/fleet.md §9
Machines pick their own nearest from the map, so nothing has to be re-issued or moved.
EOF
