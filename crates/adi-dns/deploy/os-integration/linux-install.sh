#!/usr/bin/env bash
#
# Linux: route the `adi` domain to the local resolver via systemd-resolved
# split-DNS (leaving global DNS untouched).
#
# In this split-DNS mode only `.adi` queries reach the resolver; everything else
# keeps using the system's normal upstreams, so the resolver's forwarding feature
# is unused. To make adi-dns your PRIMARY resolver (exercising forwarding too),
# set `DNS=127.0.0.1` with `Domains=~.` instead of `Domains=~adi` below, or point
# /etc/resolv.conf at 127.0.0.1.
#
# Requires systemd-resolved. For plain resolv.conf or NetworkManager+dnsmasq
# setups, see COMPARISON.md.
#
# Usage:  sudo ./linux-install.sh [DOMAIN]
set -euo pipefail

DOMAIN="${1:-adi}"

if [[ "$(id -u)" -ne 0 ]]; then
  echo "This script needs sudo." >&2
  exec sudo "$0" "$@"
fi

if ! systemctl is-active --quiet systemd-resolved; then
  echo "systemd-resolved is not active. See COMPARISON.md for resolv.conf/dnsmasq alternatives." >&2
  exit 1
fi

mkdir -p /etc/systemd/resolved.conf.d
cat > /etc/systemd/resolved.conf.d/adi.conf <<EOF
# Managed by adi-dns. Split-DNS: send only the '${DOMAIN}' domain to 127.0.0.1.
[Resolve]
DNS=127.0.0.1
Domains=~${DOMAIN}
EOF

systemctl restart systemd-resolved

echo "Installed split DNS: ~${DOMAIN} -> 127.0.0.1"
echo "Verify:  resolvectl query test.${DOMAIN}   (and: resolvectl status)"
echo "Remove:  sudo rm /etc/systemd/resolved.conf.d/adi.conf && sudo systemctl restart systemd-resolved"
