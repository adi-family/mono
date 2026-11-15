#!/usr/bin/env bash
# Runs as root ON THE INSTANCE (sent by scripts/deploy-webhook-relay-aws.sh over SSM — this box
# has no SSH, no inbound path but 80/443). Idempotent: installs Docker if missing, (re)builds the
# relay image from the extracted release, and (re)creates the three containers (relay-public,
# relay-enterprise, caddy) on a user-defined network so Caddy can reach each relay by name.
#   provision.sh <release-dir> <public-host> <ent-host>
set -euo pipefail
REL=$1 PUBLIC_HOST=$2 ENT_HOST=$3
HERE="$REL/crates/adi-relay-server/deploy"

[ "$(uname -m)" = aarch64 ] || { echo "expected an arm64 (t4g) instance" >&2; exit 1; }

command -v docker >/dev/null || dnf install -y -q docker
systemctl enable -q --now docker

install -d -m 0755 /etc/adi-webhook-relay
sed "s|__HOSTNAME__|$PUBLIC_HOST|" "$HERE/relay-public.toml.tmpl" > /etc/adi-webhook-relay/relay-public.toml
sed "s|__HOSTNAME__|$ENT_HOST|" "$HERE/relay-enterprise.toml.tmpl" > /etc/adi-webhook-relay/relay-enterprise.toml
sed -e "s|__PUBLIC_HOST__|$PUBLIC_HOST|" -e "s|__ENT_HOST__|$ENT_HOST|" "$HERE/Caddyfile.tmpl" > /etc/adi-webhook-relay/Caddyfile

docker network create adi-webhook-relay >/dev/null 2>&1 || true
for v in relay-public-data relay-enterprise-data caddy-data caddy-config; do
  docker volume create "$v" >/dev/null
done

TAG="adi-relay-server:$(basename "$REL")"
echo "building $TAG (this compiles rusqlite's bundled SQLite — a few minutes on first build)…"
docker build -q -f "$REL/crates/adi-relay-server/Dockerfile" -t "$TAG" "$REL" > /tmp/adi-relay-docker-build.log
docker image prune -f >/dev/null 2>&1 || true

for name in relay-public relay-enterprise caddy; do
  docker rm -f "$name" >/dev/null 2>&1 || true
done

docker run -d --name relay-public --restart unless-stopped --network adi-webhook-relay \
  -v relay-public-data:/data -v /etc/adi-webhook-relay/relay-public.toml:/data/relay.toml:ro \
  "$TAG" >/dev/null

docker run -d --name relay-enterprise --restart unless-stopped --network adi-webhook-relay \
  -v relay-enterprise-data:/data -v /etc/adi-webhook-relay/relay-enterprise.toml:/data/relay.toml:ro \
  "$TAG" >/dev/null

docker run -d --name caddy --restart unless-stopped --network adi-webhook-relay \
  -p 80:80 -p 443:443 \
  -v /etc/adi-webhook-relay/Caddyfile:/etc/caddy/Caddyfile:ro \
  -v caddy-data:/data -v caddy-config:/config \
  caddy:2 >/dev/null

echo "waiting for both relay containers' own /healthz…"
for i in $(seq 1 30); do
  docker exec relay-public curl -fsS http://127.0.0.1:8080/healthz >/dev/null 2>&1 \
    && docker exec relay-enterprise curl -fsS http://127.0.0.1:8080/healthz >/dev/null 2>&1 \
    && break
  sleep 2
done
docker exec relay-public curl -fsS http://127.0.0.1:8080/healthz >/dev/null \
  || { echo "relay-public never answered /healthz" >&2; docker logs --tail 40 relay-public >&2; exit 1; }
docker exec relay-enterprise curl -fsS http://127.0.0.1:8080/healthz >/dev/null \
  || { echo "relay-enterprise never answered /healthz" >&2; docker logs --tail 40 relay-enterprise >&2; exit 1; }

# Keep only the three most recent extracted releases and built images.
{ ls -1dt /opt/adi-webhook-relay/releases/* 2>/dev/null | grep -vxF "$REL" || true; } | tail -n +3 | xargs -r rm -rf

echo "containers:"; docker ps --format '  {{.Names}}: {{.Status}}'
