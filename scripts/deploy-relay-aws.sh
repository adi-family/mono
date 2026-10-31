#!/usr/bin/env bash
# Bring up one `iroh-relay` region for the fleet, on AWS EC2.
#
# The AWS sibling of `scripts/deploy-relay.sh` (GCE). Same relay, same `relay.toml`, same systemd
# unit, same ports: TCP 80 (ACME http-01), TCP 443 (the iroh-relay-v2 protocol), UDP 7842 (QUIC
# address discovery — the half that lets a direct path form; a relay behind anything that drops UDP
# is worse than no relay). Only the provider verbs differ (`aws ec2` for `gcloud`), plus one thing
# the GCE script never needed: the DNS step here can UPDATE an existing A record, not only create
# one — because the first use of this script was to MOVE the live `mad` relay off a dead GCE box
# onto EC2 by repointing its record, with every node left untouched.
#
#   scripts/deploy-relay-aws.sh mad eu-central-1
#   scripts/deploy-relay-aws.sh mad eu-central-1 --dry-run        # print, create nothing
#   scripts/deploy-relay-aws.sh iad us-east-1 --no-verify
#
# Everything is idempotent and keyed on `adi-relay-<label>` / tag role=iroh-relay, so a re-run after
# a half-finished deploy picks up where it stopped.
#
# DNS lives on Cloudflare. A Zone->DNS->Edit token is read from the secret named by --dns-secret
# (default CF_TOKEN_WITHADI). LetsEncrypt's http-01 is answered by the relay on :80, so the hostname
# must resolve to the Elastic IP BEFORE the relay first starts — hence the order below is
# address -> DNS -> wait (on the AUTHORITATIVE nameserver, not a cache) -> instance.
set -euo pipefail

command -v aws >/dev/null  || { echo "aws CLI not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 not installed" >&2; exit 1; }

IROH_VERSION="v1.0.1"        # pinned; see scripts/deploy-relay.sh for why not the workspace's iroh
DOMAIN="mono-relay.withadi.dev"
CONTACT="ihor@withadi.dev"   # LetsEncrypt refuses to start without one
INSTANCE_TYPE="t3.micro"     # the relay forwards bytes, it does not compute (e2-micro on GCE)
PROFILE="${AWS_PROFILE:-withadi}"
DNS_SECRET="CF_TOKEN_WITHADI"
SSH_KEY="adi-relay"
AMI=""                       # empty -> looked up: latest Debian 12 amd64 in the region
DISK_GB=10
DRY_RUN=0
SKIP_DNS_WAIT=0
NO_VERIFY=0

die(){ echo "error: $*" >&2; exit 1; }
usage(){ awk 'NR>1{ if($0!~/^#/) exit; sub(/^# ?/,""); print }' "${BASH_SOURCE[0]}"; echo; \
  echo "Options: --instance-type T  --ami ID  --profile P  --dns-secret NAME  --disk-gb N  --dry-run  --skip-dns-wait  --no-verify"; exit "${1:-0}"; }

[[ $# -ge 2 ]] || usage 1
LABEL="$1"; REGION="$2"; shift 2
while [[ $# -gt 0 ]]; do case "$1" in
  --instance-type) INSTANCE_TYPE="$2"; shift 2;;
  --ami) AMI="$2"; shift 2;;
  --profile) PROFILE="$2"; shift 2;;
  --dns-secret) DNS_SECRET="$2"; shift 2;;
  --disk-gb) DISK_GB="$2"; shift 2;;
  --dry-run) DRY_RUN=1; shift;;
  --skip-dns-wait) SKIP_DNS_WAIT=1; shift;;
  --no-verify) NO_VERIFY=1; shift;;
  -h|--help) usage 0;;
  *) die "unknown argument: $1";;
esac; done

[[ "$LABEL" =~ ^[a-z0-9][a-z0-9-]{0,20}$ ]] || die "label must be one lowercase DNS label: $LABEL"
FQDN="$LABEL.$DOMAIN"
NAME="adi-relay-$LABEL"
TAG="iroh-relay"
SG_NAME="adi-relay-ingress"
AWS=(aws --profile "$PROFILE" --region "$REGION")
run(){ if [[ $DRY_RUN -eq 1 ]]; then printf '  + %q' "$1"; printf ' %q' "${@:2}"; printf '\n'; else "$@"; fi; }

echo "==> relay $FQDN  ->  AWS $REGION, $INSTANCE_TYPE, iroh-relay $IROH_VERSION (profile $PROFILE)"
[[ $DRY_RUN -eq 1 ]] || "${AWS[@]}" sts get-caller-identity >/dev/null 2>&1 || die "cannot auth to AWS (profile $PROFILE). Try: aws sso login / aws configure --profile $PROFILE"

VPC="$("${AWS[@]}" ec2 describe-vpcs --filters Name=isDefault,Values=true --query 'Vpcs[0].VpcId' --output text)"
[[ -n "$VPC" && "$VPC" != "None" ]] || die "no default VPC in $REGION"
SUBNET="$("${AWS[@]}" ec2 describe-subnets --filters Name=vpc-id,Values="$VPC" Name=default-for-az,Values=true --query 'Subnets[0].SubnetId' --output text)"
[[ -n "$SUBNET" && "$SUBNET" != "None" ]] || die "no default subnet in $REGION"
if [[ -z "$AMI" ]]; then
  AMI="$("${AWS[@]}" ssm get-parameters --names /aws/service/debian/release/12/latest/amd64 --query 'Parameters[0].Value' --output text 2>/dev/null || true)"
  [[ -n "$AMI" && "$AMI" != "None" ]] || AMI="$("${AWS[@]}" ec2 describe-images --owners 136693071363 --filters 'Name=name,Values=debian-12-amd64-*' 'Name=state,Values=available' --query 'reverse(sort_by(Images,&CreationDate))[0].ImageId' --output text)"
fi
[[ -n "$AMI" && "$AMI" != "None" ]] || die "could not resolve a Debian 12 AMI in $REGION"
echo "    vpc=$VPC subnet=$SUBNET ami=$AMI"

# ---------------------------------------------------------------------------- 1. the address
echo "==> elastic IP $NAME"
ALLOC="$("${AWS[@]}" ec2 describe-addresses --filters Name=tag:Name,Values="$NAME" --query 'Addresses[0].AllocationId' --output text 2>/dev/null || true)"
if [[ -z "$ALLOC" || "$ALLOC" == "None" ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then run "${AWS[@]}" ec2 allocate-address --domain vpc --tag-specifications "ResourceType=elastic-ip,Tags=[{Key=Name,Value=$NAME},{Key=role,Value=$TAG}]"; ALLOC="dry"; \
  else ALLOC="$("${AWS[@]}" ec2 allocate-address --domain vpc --tag-specifications "ResourceType=elastic-ip,Tags=[{Key=Name,Value=$NAME},{Key=role,Value=$TAG}]" --query AllocationId --output text)"; fi
fi
if [[ $DRY_RUN -eq 1 ]]; then IP="<reserved-ip>"; else IP="$("${AWS[@]}" ec2 describe-addresses --allocation-ids "$ALLOC" --query 'Addresses[0].PublicIp' --output text)"; fi
[[ -n "$IP" ]] || die "could not read back the elastic IP"
echo "    address: $IP ($ALLOC)"

# ---------------------------------------------------------------------------- 2. the firewall
echo "==> security group $SG_NAME"
SG="$("${AWS[@]}" ec2 describe-security-groups --filters Name=group-name,Values="$SG_NAME" Name=vpc-id,Values="$VPC" --query 'SecurityGroups[0].GroupId' --output text 2>/dev/null || true)"
if [[ -z "$SG" || "$SG" == "None" ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then echo "  + create-security-group $SG_NAME (tcp 22,80,443 + udp 7842 from 0.0.0.0/0, ::/0)"; SG="dry"; \
  else
    SG="$("${AWS[@]}" ec2 create-security-group --group-name "$SG_NAME" --description 'iroh-relay: ACME :80, protocol :443, QUIC udp:7842, mgmt :22' --vpc-id "$VPC" --query GroupId --output text)"
    for spec in 'tcp:22' 'tcp:80' 'tcp:443' 'udp:7842'; do
      p="${spec%:*}"; port="${spec#*:}"
      "${AWS[@]}" ec2 authorize-security-group-ingress --group-id "$SG" --ip-permissions "IpProtocol=$p,FromPort=$port,ToPort=$port,IpRanges=[{CidrIp=0.0.0.0/0}],Ipv6Ranges=[{CidrIpv6=::/0}]" >/dev/null
    done
  fi
fi
echo "    $SG"

# ---------------------------------------------------------------------------- 3. a key pair (debug)
# The relay provisions itself from user-data; SSH is only for when it does not. Harmless if absent.
KEYARG=()
if "${AWS[@]}" ec2 describe-key-pairs --key-names "$SSH_KEY" >/dev/null 2>&1; then
  KEYARG=(--key-name "$SSH_KEY")
elif [[ $DRY_RUN -eq 0 ]]; then
  pem="$HOME/.ssh/$SSH_KEY.pem"
  if "${AWS[@]}" ec2 create-key-pair --key-name "$SSH_KEY" --query KeyMaterial --output text > "$pem" 2>/dev/null; then
    chmod 600 "$pem"; KEYARG=(--key-name "$SSH_KEY"); echo "    created key pair $SSH_KEY -> $pem"
  else rm -f "$pem"; echo "    (no key pair; continuing without SSH)"; fi
fi

# ---------------------------------------------------------------------------- 4. the DNS record
echo "==> DNS: $FQDN must resolve to $IP before the relay first starts (grey cloud / unproxied)"
cf_token="$(adi-mono secrets read "$DNS_SECRET" 2>/dev/null || true)"
auth_ip(){ local ns; ns="$(dig +short NS "${DOMAIN#*.}" | head -1)"; [[ -n "$ns" ]] && dig +short A "$FQDN" @"$ns" | tail -1 || dig +short A "$FQDN" | tail -1; }
publish_dns(){
  [[ -n "$cf_token" ]] || return 1
  local zone_name zid rid body
  zone_name="${DOMAIN#*.}"
  zid="$(curl -fsS -H "Authorization: Bearer $cf_token" "https://api.cloudflare.com/client/v4/zones?name=$zone_name" | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r[0]["id"] if r else "")')"
  [[ -n "$zid" ]] || return 1
  rid="$(curl -fsS -H "Authorization: Bearer $cf_token" "https://api.cloudflare.com/client/v4/zones/$zid/dns_records?type=A&name=$FQDN" | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r[0]["id"] if r else "")')"
  body="{\"type\":\"A\",\"name\":\"$FQDN\",\"content\":\"$IP\",\"proxied\":false,\"ttl\":300}"
  if [[ -n "$rid" ]]; then
    curl -fsS -X PUT -H "Authorization: Bearer $cf_token" -H 'Content-Type: application/json' "https://api.cloudflare.com/client/v4/zones/$zid/dns_records/$rid" -d "$body" >/dev/null
  else
    curl -fsS -X POST -H "Authorization: Bearer $cf_token" -H 'Content-Type: application/json' "https://api.cloudflare.com/client/v4/zones/$zid/dns_records" -d "$body" >/dev/null
  fi
}
if [[ $DRY_RUN -eq 1 ]]; then
  echo "    (dry run — would set A $FQDN -> $IP, unproxied)"
elif [[ "$(auth_ip)" == "$IP" ]]; then
  echo "    authoritative already serves $IP"
elif publish_dns; then
  echo "    set via the Cloudflare API"
else
  cat >&2 <<EOF
    Cannot write DNS (no secret $DNS_SECRET, or the API refused). Add it by hand, then re-run:
      withadi.dev -> DNS -> A   $LABEL.${DOMAIN%%.*}   ->   $IP   (DNS only / grey cloud, TTL auto)
    To automate: mint a Zone->DNS->Edit token on withadi.dev and  adi-mono secrets set $DNS_SECRET
EOF
  exit 2
fi
if [[ $DRY_RUN -eq 0 && $SKIP_DNS_WAIT -eq 0 ]]; then
  echo -n "    waiting for the authoritative nameserver to serve $IP"
  for _ in $(seq 1 40); do [[ "$(auth_ip)" == "$IP" ]] && { echo " ok"; break; }; echo -n "."; sleep 3; done
  [[ "$(auth_ip)" == "$IP" ]] || die "$FQDN still not $IP authoritatively; LetsEncrypt would fail"
fi

# ---------------------------------------------------------------------------- 5. the instance
# user-data provisions the box exactly as adi-relay-mad on GCE — same binary, unit and relay.toml.
startup="$(cat <<EOF
#!/usr/bin/env bash
set -euo pipefail
arch="\$(uname -m)"
case "\$arch" in x86_64) target=x86_64 ;; aarch64) target=aarch64 ;; *) echo "unsupported: \$arch" >&2; exit 1 ;; esac
if [[ -x /usr/local/bin/iroh-relay && -f /etc/iroh-relay/relay.toml ]]; then exit 0; fi
url="https://github.com/n0-computer/iroh/releases/download/$IROH_VERSION/iroh-relay-$IROH_VERSION-\${target}-unknown-linux-musl.tar.gz"
tmp="\$(mktemp -d)"
curl -fsSL --retry 5 --retry-delay 3 "\$url" -o "\$tmp/relay.tgz"
tar -xzf "\$tmp/relay.tgz" -C "\$tmp"
install -m 0755 "\$(find "\$tmp" -type f -name iroh-relay | head -1)" /usr/local/bin/iroh-relay
rm -rf "\$tmp"
id -u iroh-relay >/dev/null 2>&1 || useradd --system --no-create-home --shell /usr/sbin/nologin iroh-relay
install -d -o iroh-relay -g iroh-relay -m 0755 /var/lib/iroh-relay /var/lib/iroh-relay/certs
install -d -m 0755 /etc/iroh-relay
cat >/etc/iroh-relay/relay.toml <<'CONF'
# iroh-relay for the adi fleet — stateless, untrusted by construction (peer identity is Ed25519,
# verified end to end). Nothing on disk but the ACME certificate.
enable_relay = true
http_bind_addr = "[::]:80"
enable_quic_addr_discovery = true

[tls]
hostname = ["$FQDN"]
cert_mode = "LetsEncrypt"
contact = "$CONTACT"
prod_tls = true
cert_dir = "/var/lib/iroh-relay/certs"
https_bind_addr = "[::]:443"
quic_bind_addr = "[::]:7842"

# Open on purpose (a fresh install pairs with no config), metered because of it.
[limits.client.rx]
bytes_per_second = 1048576
max_burst_bytes = 4194304
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

echo "==> instance $NAME"
IID="$("${AWS[@]}" ec2 describe-instances --filters Name=tag:Name,Values="$NAME" Name=instance-state-name,Values=pending,running,stopping,stopped --query 'Reservations[0].Instances[0].InstanceId' --output text 2>/dev/null || true)"
if [[ -n "$IID" && "$IID" != "None" ]]; then
  echo "    already exists ($IID) — leaving it alone"
elif [[ $DRY_RUN -eq 1 ]]; then
  echo "  + run-instances --image-id $AMI --instance-type $INSTANCE_TYPE --subnet-id $SUBNET --security-group-ids $SG (+ user-data, tag $NAME); then associate $IP"
else
  udf="$(mktemp)"; printf '%s\n' "$startup" > "$udf"
  IID="$("${AWS[@]}" ec2 run-instances --image-id "$AMI" --instance-type "$INSTANCE_TYPE" \
    --subnet-id "$SUBNET" --security-group-ids "$SG" "${KEYARG[@]}" \
    --metadata-options 'HttpTokens=required,HttpEndpoint=enabled' \
    --block-device-mappings "DeviceName=/dev/xvda,Ebs={VolumeSize=$DISK_GB,VolumeType=gp3}" \
    --user-data "file://$udf" \
    --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$NAME},{Key=role,Value=$TAG}]" \
    --query 'Instances[0].InstanceId' --output text)"
  rm -f "$udf"
  echo "    launched $IID — waiting for running"
  "${AWS[@]}" ec2 wait instance-running --instance-ids "$IID"
  "${AWS[@]}" ec2 associate-address --allocation-id "$ALLOC" --instance-id "$IID" >/dev/null
  echo "    associated EIP $IP"
fi

# ---------------------------------------------------------------------------- 6. verify
[[ $DRY_RUN -eq 1 ]] && { echo "==> dry run complete"; exit 0; }
[[ $NO_VERIFY -eq 1 ]] && { echo "==> launched; skipping verify (--no-verify). Check: curl -fsS https://$FQDN/"; exit 0; }
echo "==> waiting for boot, install and the LetsEncrypt certificate (up to ~6 min)"
for _ in $(seq 1 72); do curl -fsS -m 5 "https://$FQDN/" 2>/dev/null | grep -qi 'iroh relay' && break; echo -n "."; sleep 5; done; echo
echo "--- https://$FQDN/"; curl -fsS -m 10 "https://$FQDN/" | head -3 || die "the relay is not answering over HTTPS"
echo "--- websocket upgrade"
ws="$(curl -sS -m 5 -D - -o /dev/null -H 'Connection: Upgrade' -H 'Upgrade: websocket' -H 'Sec-WebSocket-Version: 13' -H 'Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==' -H 'Sec-WebSocket-Protocol: iroh-relay-v2' "https://$FQDN/relay" 2>/dev/null || true)"
grep -qi '^sec-websocket-protocol: iroh-relay-v2' <<<"$ws" && echo "    101 echoes iroh-relay-v2 — browser-compatible" || die "upgrade did not echo iroh-relay-v2"
echo "--- UDP 7842 (QUIC address discovery)"; nc -z -u -w 5 "$FQDN" 7842 && echo "    reachable" || echo "    WARNING: no UDP answer — direct paths will not form"
echo; echo "==> $FQDN is live at $IP ($IID)"
