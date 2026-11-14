#!/usr/bin/env bash
# Bring up one webhook-relay region on AWS EC2 (ADI-MONO-224).
#
# One machine per region, two Docker containers on it (mode=public, mode=enterprise — separate
# relay.toml, db_path and volume each) with Caddy in front terminating TLS and routing by
# hostname. No SSH, no key pair: the instance has only an SSM-managed instance profile and its
# security group opens tcp 80/443 only — everything after boot happens over
# `aws ssm send-command`. This box has no Docker to build the image locally, so the image is
# built ON THE INSTANCE itself, from the committed tree shipped as `git archive` -> S3 -> pulled
# and extracted over SSM — the same shape adi-seller-portal's scripts/deploy.sh uses, mirrored
# here rather than scripts/deploy-relay-aws.sh's own user-data-only shape (that script's iroh
# binary is one static download; this one is a multi-crate Rust workspace member that needs a
# real build, so "ship source, build remotely" fits better than "ship a prebuilt binary").
#
# Usage:
#   scripts/deploy-webhook-relay-aws.sh eu-central-1
#   scripts/deploy-webhook-relay-aws.sh us-east-1
#
# Everything is idempotent and keyed on tag Name=adi-webhook-relay-<label> / Project=adi-webhook-relay
# (every resource this script creates carries Project=adi-webhook-relay — nothing else is touched),
# so a re-run after a half-finished deploy, or a later "redeploy this region" after a `git commit`,
# picks up where it stopped and rebuilds the image from whatever is at HEAD.
#
# DNS lives on Cloudflare (CF_TOKEN, zone withadi.dev — NOT CLOUDFLARE_API_TOKEN, which has no DNS
# edit on this account). Each region's two hostnames must resolve to its Elastic IP BEFORE Caddy
# first starts, because its ACME http-01 challenge is answered on :80 by Caddy itself — hence the
# order below is address -> DNS -> wait (on the authoritative nameserver) -> instance -> ship.
set -euo pipefail

command -v aws >/dev/null || { echo "aws CLI not installed" >&2; exit 1; }
command -v python3 >/dev/null || { echo "python3 not installed" >&2; exit 1; }
command -v git >/dev/null || { echo "git not installed" >&2; exit 1; }

DOMAIN="withadi.dev"
# Fixed, not looked up: this bucket and this role are owned by this deploy, created once
# (2026-11-14) and reused by every region/redeploy since — see the task's own report for how.
BUCKET="adi-webhook-relay-releases-4ca4663be983"
INSTANCE_PROFILE="adi-webhook-relay-ssm"
INSTANCE_TYPE="t4g.small"   # arm64: ~20% cheaper than t3.small at this size, and the relay is a
                            # thin I/O-bound Rust binary — no architecture-specific dependency
DISK_GB=20                  # headroom for the Docker build (rustc + the image layers)
PROFILE="${AWS_PROFILE:-}"
DRY_RUN=0

die(){ echo "error: $*" >&2; exit 1; }
usage(){ awk 'NR>1{ if($0!~/^#/) exit; sub(/^# ?/,""); print }' "${BASH_SOURCE[0]}"; exit "${1:-0}"; }

[[ $# -ge 1 ]] || usage 1
REGION="$1"; shift || true
while [[ $# -gt 0 ]]; do case "$1" in
  --dry-run) DRY_RUN=1; shift;;
  -h|--help) usage 0;;
  *) die "unknown argument: $1";;
esac; done

case "$REGION" in
  eu-central-1) LABEL=eu ;;
  us-east-1)    LABEL=us ;;
  *) die "region must be eu-central-1 or us-east-1 (got $REGION)" ;;
esac
PUBLIC_HOST="$LABEL.webhooks.$DOMAIN"
ENT_HOST="$LABEL.ent.webhooks.$DOMAIN"
NAME="adi-webhook-relay-$LABEL"
SG_NAME="adi-webhook-relay-ingress"
AWS=(aws --region "$REGION"); [[ -n "$PROFILE" ]] && AWS=(aws --profile "$PROFILE" --region "$REGION")

cd "$(git rev-parse --show-toplevel)"

echo "==> webhook relay $LABEL ($PUBLIC_HOST, $ENT_HOST) -> AWS $REGION, $INSTANCE_TYPE"
[[ $DRY_RUN -eq 1 ]] || "${AWS[@]}" sts get-caller-identity >/dev/null 2>&1 || die "cannot auth to AWS in $REGION"

VPC="$("${AWS[@]}" ec2 describe-vpcs --filters Name=isDefault,Values=true --query 'Vpcs[0].VpcId' --output text)"
[[ -n "$VPC" && "$VPC" != "None" ]] || die "no default VPC in $REGION"
SUBNET="$("${AWS[@]}" ec2 describe-subnets --filters Name=vpc-id,Values="$VPC" Name=default-for-az,Values=true --query 'Subnets[0].SubnetId' --output text)"
[[ -n "$SUBNET" && "$SUBNET" != "None" ]] || die "no default subnet in $REGION"
AMI="$("${AWS[@]}" ssm get-parameters --names /aws/service/ami-amazon-linux-latest/al2023-ami-kernel-default-arm64 --query 'Parameters[0].Value' --output text)"
[[ -n "$AMI" && "$AMI" != "None" ]] || die "could not resolve an AL2023 arm64 AMI in $REGION"
echo "    vpc=$VPC subnet=$SUBNET ami=$AMI"

# ---------------------------------------------------------------------------- 1. the address
echo "==> elastic IP $NAME"
ALLOC="$("${AWS[@]}" ec2 describe-addresses --filters Name=tag:Name,Values="$NAME" --query 'Addresses[0].AllocationId' --output text 2>/dev/null || true)"
if [[ -z "$ALLOC" || "$ALLOC" == "None" ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then ALLOC="dry"; else
    ALLOC="$("${AWS[@]}" ec2 allocate-address --domain vpc --tag-specifications "ResourceType=elastic-ip,Tags=[{Key=Name,Value=$NAME},{Key=Project,Value=adi-webhook-relay}]" --query AllocationId --output text)"
  fi
fi
if [[ $DRY_RUN -eq 1 ]]; then IP="<reserved-ip>"; else IP="$("${AWS[@]}" ec2 describe-addresses --allocation-ids "$ALLOC" --query 'Addresses[0].PublicIp' --output text)"; fi
echo "    address: $IP ($ALLOC)"

# ---------------------------------------------------------------------------- 2. the firewall
echo "==> security group $SG_NAME (tcp 80,443 only — no SSH, operate via SSM)"
SG="$("${AWS[@]}" ec2 describe-security-groups --filters Name=group-name,Values="$SG_NAME" Name=vpc-id,Values="$VPC" --query 'SecurityGroups[0].GroupId' --output text 2>/dev/null || true)"
if [[ -z "$SG" || "$SG" == "None" ]]; then
  if [[ $DRY_RUN -eq 1 ]]; then SG="dry"; else
    SG="$("${AWS[@]}" ec2 create-security-group --group-name "$SG_NAME" --description 'adi-webhook-relay: tcp 80 (ACME+ingress), 443 (ingress) - no SSH, SSM only' --vpc-id "$VPC" --tag-specifications "ResourceType=security-group,Tags=[{Key=Project,Value=adi-webhook-relay}]" --query GroupId --output text)"
    for port in 80 443; do
      "${AWS[@]}" ec2 authorize-security-group-ingress --group-id "$SG" --ip-permissions "IpProtocol=tcp,FromPort=$port,ToPort=$port,IpRanges=[{CidrIp=0.0.0.0/0}],Ipv6Ranges=[{CidrIpv6=::/0}]" >/dev/null
    done
  fi
fi
echo "    $SG"

# ---------------------------------------------------------------------------- 3. the SSM role
echo "==> instance profile $INSTANCE_PROFILE"
if ! aws iam get-instance-profile --instance-profile-name "$INSTANCE_PROFILE" >/dev/null 2>&1; then
  if [[ $DRY_RUN -eq 1 ]]; then echo "  + create-role/instance-profile $INSTANCE_PROFILE (AmazonSSMManagedInstanceCore + s3:GetObject on $BUCKET)"; else
    die "instance profile $INSTANCE_PROFILE is missing — create it once (see the task's report for the exact commands) before running this script"
  fi
fi

# ---------------------------------------------------------------------------- 4. the DNS records
echo "==> DNS: $PUBLIC_HOST and $ENT_HOST must resolve to $IP before Caddy's first start"
cf_token="$(adi-mono secrets read CF_TOKEN 2>/dev/null || true)"
[[ -n "$cf_token" ]] || die "no CF_TOKEN secret (adi-mono secrets set CF_TOKEN)"
auth_ip(){ local host="$1" ns; ns="$(dig +short NS "$DOMAIN" | head -1)"; [[ -n "$ns" ]] && dig +short A "$host" @"$ns" | tail -1 || dig +short A "$host" | tail -1; }
upsert_dns(){
  local host="$1" zid rid body
  zid="$(curl -fsS -H "Authorization: Bearer $cf_token" "https://api.cloudflare.com/client/v4/zones?name=$DOMAIN" | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r[0]["id"] if r else "")')"
  [[ -n "$zid" ]] || return 1
  rid="$(curl -fsS -H "Authorization: Bearer $cf_token" "https://api.cloudflare.com/client/v4/zones/$zid/dns_records?type=A&name=$host" | python3 -c 'import json,sys;r=json.load(sys.stdin)["result"];print(r[0]["id"] if r else "")')"
  body="{\"type\":\"A\",\"name\":\"$host\",\"content\":\"$IP\",\"proxied\":false,\"ttl\":300,\"comment\":\"adi-webhook-relay $LABEL ($REGION)\"}"
  if [[ -n "$rid" ]]; then curl -fsS -X PUT -H "Authorization: Bearer $cf_token" -H 'Content-Type: application/json' "https://api.cloudflare.com/client/v4/zones/$zid/dns_records/$rid" -d "$body" >/dev/null
  else curl -fsS -X POST -H "Authorization: Bearer $cf_token" -H 'Content-Type: application/json' "https://api.cloudflare.com/client/v4/zones/$zid/dns_records" -d "$body" >/dev/null
  fi
}
for host in "$PUBLIC_HOST" "$ENT_HOST"; do
  if [[ $DRY_RUN -eq 1 ]]; then echo "    (dry run — would set A $host -> $IP, unproxied)"; continue; fi
  if [[ "$(auth_ip "$host")" == "$IP" ]]; then echo "    $host already authoritative $IP"
  elif upsert_dns "$host"; then echo "    $host -> $IP (set via Cloudflare API)"
  else die "could not set DNS for $host (CF_TOKEN refused or zone lookup failed)"
  fi
done
if [[ $DRY_RUN -eq 0 ]]; then
  for host in "$PUBLIC_HOST" "$ENT_HOST"; do
    echo -n "    waiting for the authoritative nameserver to serve $host -> $IP"
    for _ in $(seq 1 40); do [[ "$(auth_ip "$host")" == "$IP" ]] && { echo " ok"; break; }; echo -n "."; sleep 3; done
    [[ "$(auth_ip "$host")" == "$IP" ]] || die "$host still not $IP authoritatively; LetsEncrypt would fail"
  done
fi

# ---------------------------------------------------------------------------- 5. the instance
echo "==> instance $NAME"
IID="$("${AWS[@]}" ec2 describe-instances --filters Name=tag:Name,Values="$NAME" Name=instance-state-name,Values=pending,running,stopping,stopped --query 'Reservations[0].Instances[0].InstanceId' --output text 2>/dev/null || true)"
if [[ -n "$IID" && "$IID" != "None" ]]; then
  echo "    already exists ($IID) — leaving it alone, shipping a fresh release to it"
elif [[ $DRY_RUN -eq 1 ]]; then
  echo "  + run-instances --image-id $AMI --instance-type $INSTANCE_TYPE --subnet-id $SUBNET --security-group-ids $SG --iam-instance-profile $INSTANCE_PROFILE (EBS encrypted ${DISK_GB}G, tag $NAME); then associate $IP"
  echo "==> dry run complete"; exit 0
else
  IID="$("${AWS[@]}" ec2 run-instances --image-id "$AMI" --instance-type "$INSTANCE_TYPE" \
    --subnet-id "$SUBNET" --security-group-ids "$SG" \
    --iam-instance-profile "Name=$INSTANCE_PROFILE" \
    --metadata-options 'HttpTokens=required,HttpEndpoint=enabled' \
    --block-device-mappings "DeviceName=/dev/xvda,Ebs={VolumeSize=$DISK_GB,VolumeType=gp3,Encrypted=true}" \
    --tag-specifications "ResourceType=instance,Tags=[{Key=Name,Value=$NAME},{Key=Project,Value=adi-webhook-relay}]" "ResourceType=volume,Tags=[{Key=Name,Value=$NAME},{Key=Project,Value=adi-webhook-relay}]" \
    --query 'Instances[0].InstanceId' --output text)"
  echo "    launched $IID — waiting for running"
  "${AWS[@]}" ec2 wait instance-running --instance-ids "$IID"
  "${AWS[@]}" ec2 associate-address --allocation-id "$ALLOC" --instance-id "$IID" >/dev/null
  echo "    associated EIP $IP"
fi

echo -n "    waiting for the SSM agent to register"
for _ in $(seq 1 40); do
  state="$("${AWS[@]}" ssm describe-instance-information --filters "Key=InstanceIds,Values=$IID" --query 'InstanceInformationList[0].PingStatus' --output text 2>/dev/null || true)"
  [[ "$state" == "Online" ]] && { echo " ok"; break; }
  echo -n "."; sleep 5
done
[[ "$state" == "Online" ]] || die "$IID's SSM agent never came Online — can't ship without it (no SSH on this box)"

# ---------------------------------------------------------------------------- 6. ship the release
ssm_run() {
  local script=$1 cid status
  cid=$("${AWS[@]}" ssm send-command --instance-ids "$IID" --document-name AWS-RunShellScript \
    --comment "adi-webhook-relay $LABEL" --timeout-seconds 600 \
    --parameters "$(python3 -c 'import json,sys;print(json.dumps({"commands":[sys.argv[1]],"executionTimeout":["1800"]}))' "$script")" \
    --query Command.CommandId --output text)
  while :; do
    sleep 3
    status="$("${AWS[@]}" ssm get-command-invocation --command-id "$cid" --instance-id "$IID" --query Status --output text 2>/dev/null || echo Pending)"
    case "$status" in Pending|InProgress|Delayed) continue ;; esac
    break
  done
  local out; out="$("${AWS[@]}" ssm get-command-invocation --command-id "$cid" --instance-id "$IID" --output json)"
  python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["StandardOutputContent"],end="")' <<<"$out"
  python3 -c 'import json,sys;d=json.load(sys.stdin);e=d["StandardErrorContent"];print(e,end="",file=sys.stderr) if e else None' <<<"$out"
  [[ "$status" == Success ]] || { echo "ssm command $cid: $status" >&2; return 1; }
}

[ -z "$(git status --porcelain)" ] || echo "warning: uncommitted changes are not deployed — shipping HEAD only" >&2
SHA="$(git rev-parse --short=12 HEAD)"
echo "==> shipping release $SHA"
tmp="$(mktemp -d)"; trap 'rm -rf "$tmp"' EXIT
git archive --format=tar.gz -o "$tmp/release.tar.gz" HEAD
aws s3 cp --only-show-errors --region eu-central-1 "$tmp/release.tar.gz" "s3://$BUCKET/releases/$SHA.tar.gz"

echo "==> installing on $IID (this builds the Docker image remotely — this box has no Docker; ~3-6 min on first build)"
ssm_run "set -eu
command -v aws >/dev/null || { dnf install -y -q unzip >/dev/null; curl -fsS https://awscli.amazonaws.com/awscli-exe-linux-aarch64.zip -o /tmp/awscliv2.zip; cd /tmp && unzip -q awscliv2.zip && ./aws/install; }
d=/opt/adi-webhook-relay/releases/$SHA
rm -rf \"\$d\" && mkdir -p \"\$d\"
aws s3 cp --only-show-errors --region eu-central-1 s3://$BUCKET/releases/$SHA.tar.gz /tmp/release-$SHA.tar.gz
tar -xzf /tmp/release-$SHA.tar.gz -C \"\$d\" && rm -f /tmp/release-$SHA.tar.gz
bash \"\$d/crates/adi-relay-server/deploy/provision.sh\" \"\$d\" $PUBLIC_HOST $ENT_HOST"

# ---------------------------------------------------------------------------- 7. verify
echo "==> verifying over TLS (allowing a minute for the certificate)"
for host in "$PUBLIC_HOST" "$ENT_HOST"; do
  ok=0
  for _ in $(seq 1 20); do
    body="$(curl -fsS -m 10 "https://$host/healthz" 2>/dev/null || true)"
    [[ "$body" == "ok" ]] && { ok=1; break; }
    sleep 3
  done
  [[ $ok -eq 1 ]] || die "https://$host/healthz never answered 'ok'"
  echo "    https://$host/healthz -> ok"
done
echo "==> $LABEL ($REGION) live: $IP ($IID) — $PUBLIC_HOST, $ENT_HOST"
