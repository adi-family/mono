#!/usr/bin/env bash
# Spin a fresh Linux "node" container and pair it with this machine, end to end.
#
# Everything the demo needs in one command. Prints the hostnames to open and the password.
#
#   ./run.sh [container-name]
#
# The three docker flags that matter, and why (each was learned the hard way):
#   --cap-add SYS_ADMIN ... not --privileged: privileged grants every capability, which makes
#                           `setcap` look unnecessary and hides the real front-door behaviour.
#   --sysctl net.ipv4.ip_unprivileged_port_start=1024
#                           Docker's default is 0, so any user binds :80 and the capability is
#                           never exercised. 1024 restores how a real machine behaves.
#   --cgroupns=host + /sys/fs/cgroup
#                           systemd needs its own cgroup view to run as PID 1.
set -euo pipefail

NAME="${1:-adi-node}"
# A throwaway password for a throwaway container: this node exists to be thrown away, and a
# generated one would have to be scraped back out of the install log every run. Override it for
# anything that outlives the test.
PASSWORD="${ADI_TEST_PASSWORD:-adi-test}"
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
PKG="$ROOT/apps/linux/build/adi-linux-x64"
ADI_MONO="$ROOT/target/release/adi-mono"

[ -d "$PKG" ] || { echo "error: no package at $PKG — run: bash apps/linux/build.sh" >&2; exit 1; }

echo "==> (re)creating $NAME"
docker rm -f "$NAME" >/dev/null 2>&1 || true
docker run -d --name "$NAME" --platform linux/amd64 \
    --cap-add SYS_ADMIN --cgroupns=host \
    --tmpfs /run --tmpfs /run/lock \
    -v /sys/fs/cgroup:/sys/fs/cgroup:rw \
    --sysctl net.ipv4.ip_unprivileged_port_start=1024 \
    adi-node-test >/dev/null

echo "==> waiting for systemd"
until docker exec "$NAME" systemctl is-system-running 2>/dev/null | grep -qE 'running|degraded'; do sleep 2; done
docker exec "$NAME" loginctl enable-linger adi >/dev/null
until docker exec "$NAME" test -d /run/user/1000; do sleep 1; done

echo "==> installing and pairing"
docker cp "$PKG" "$NAME:/tmp/pkg" >/dev/null
"$ADI_MONO" mesh invite --ttl 30 2>/dev/null | head -1 > /tmp/adi-invite.txt
docker cp /tmp/adi-invite.txt "$NAME:/tmp/token.txt" >/dev/null
docker exec "$NAME" chown -R adi:adi /tmp/pkg /tmp/token.txt

docker exec -u adi -w /tmp/pkg "$NAME" sh -c \
    'export XDG_RUNTIME_DIR=/run/user/$(id -u); ./install.sh "$(cat /tmp/token.txt)"' >/tmp/adi-install.log 2>&1 \
    || { echo "install failed — see /tmp/adi-install.log" >&2; tail -20 /tmp/adi-install.log >&2; exit 1; }

NODE="$(docker exec -u adi "$NAME" sh -c 'export PATH=$HOME/.local/adi/bin:$PATH; adi-mono mesh name')"

# The front door needs the capability to bind :80, and a known password beats the one-shot one
# printed at pairing when a demo may need to type it twice.
docker exec "$NAME" setcap 'cap_net_bind_service=+ep' /home/adi/.local/adi/bin/adi-hive
docker exec -u adi "$NAME" sh -c \
    'export XDG_RUNTIME_DIR=/run/user/$(id -u); export PATH=$HOME/.local/adi/bin:$PATH; adi-mono up' >/dev/null 2>&1
VIEWER="$(docker exec -u adi "$NAME" sh -c 'export PATH=$HOME/.local/adi/bin:$PATH; adi-mono mesh fleet' | head -1 | cut -d' ' -f1)"
docker exec -u adi "$NAME" sh -c \
    "export PATH=\$HOME/.local/adi/bin:\$PATH; adi-mono mesh passwd $VIEWER --password $PASSWORD" >/dev/null
docker exec -u adi "$NAME" sh -c \
    "export PATH=\$HOME/.local/adi/bin:\$PATH; adi-mono mesh grant $VIEWER 'http:*'" >/dev/null

echo "==> waiting for the node to answer over the mesh"
until [ "$(curl -s -o /dev/null -w '%{http_code}' --max-time 20 -u adi:$PASSWORD "http://app.$NODE.n.adi/api/health")" = "200" ]; do sleep 3; done

cat <<EOF

    node:     $NODE
    open:     http://app.$NODE.n.adi/      (its control panel)
    login:    adi / $PASSWORD

    Create a dashboard on it from that panel, then open it at
              http://<name>.$NODE.n.adi/

EOF
