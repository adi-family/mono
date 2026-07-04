#!/usr/bin/env bash
#
# macOS: route the `.adi` domain to the local resolver (split-DNS mode).
#
# macOS reads per-domain resolver files from /etc/resolver/<domain>. Only names
# ending in `.adi` are sent to 127.0.0.1; all other DNS is untouched. This means
# the resolver's *forwarding* feature is unused in split-DNS mode — it only ever
# sees `.adi` queries. To use adi-dns as your PRIMARY resolver (so its forwarding
# is exercised too), instead set 127.0.0.1 as the DNS server for your network
# service:  networksetup -setdnsservers Wi-Fi 127.0.0.1
#
# Usage:  sudo ./macos-install.sh [DOMAIN] [PORT]
set -euo pipefail

DOMAIN="${1:-adi}"
PORT="${2:-53}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "This script needs sudo (writes to /etc/resolver)." >&2
  exec sudo "$0" "$@"
fi

mkdir -p /etc/resolver
resolver_file="/etc/resolver/${DOMAIN}"

{
  echo "nameserver 127.0.0.1"
  # `port` is only needed when the resolver listens somewhere other than 53.
  if [[ "$PORT" != "53" ]]; then echo "port ${PORT}"; fi
} > "$resolver_file"

# Pick up the new resolver immediately.
dscacheutil -flushcache || true
killall -HUP mDNSResponder || true

echo "Installed ${resolver_file} -> 127.0.0.1:${PORT} for *.${DOMAIN}"
echo "Verify:  dscacheutil -q host -a name test.${DOMAIN}"
echo "Remove:  sudo rm ${resolver_file} && sudo killall -HUP mDNSResponder"
