#!/usr/bin/env bash
#
# Stand up the **demo node**: a GCE box running a full adi node, kept online so somebody who owns
# no machine can still be shown a fleet. Written for App Review — Guideline 2.1 asks that a
# reviewer be able to *use* the app, and adi Fleet shows nothing until it is paired with something
# (`apps/ios/APPSTORE.md` §2.2) — but it is the same box you would want for a demo call or a
# landing-page video.
#
#   scripts/deploy-demo-node.sh                          # create it, pair it with this Mac
#   scripts/deploy-demo-node.sh --dry-run                # print every command, run none
#   scripts/deploy-demo-node.sh --zone us-east4-a        # somewhere else
#   scripts/deploy-demo-node.sh --no-pair                # create it; pair it yourself later
#
# ## How this differs from deploy-relay.sh, and why it is so much shorter
#
# A relay is a public server: it needs a static address, a DNS record, a certificate and an open
# :80/:443/udp:7842. **A node needs none of that.** `docs/fleet.md` §6 is the whole reason — a
# node's only network-facing action is an *outbound* QUIC session, so it listens on nothing and is
# reached back down the session it opened. So there is no address to reserve, no firewall rule to
# add, no DNS to publish and nothing to wait for. It does keep an ephemeral external IP, purely for
# egress: it has to fetch the release tarball and reach a relay.
#
# The corollary is worth saying out loud: **nothing here opens :22 either.** If you can ssh to this
# box it is because the project's own `default-allow-ssh` rule covers it, not because this script
# asked for it. `gcloud compute ssh` still works, IAM-gated, and is how you debug the thing.
#
# ## What ends up on the VM
#
# Debian 12, and the released `adi-linux-x64.tar.gz` — the same tarball a human would download,
# installed by its own `install.sh`, which is the only thing that knows how a node is laid out.
# This script does not template a systemd unit or copy a binary; `adi-core`'s supervisor owns that
# (`apps/linux/README.md`), and a second opinion about it here would be a second thing to keep in
# step.
#
# It installs as an unprivileged user with logind **lingering** enabled, because every adi service
# runs under `systemd --user` and a headless box has no session to keep them alive otherwise.
#
# ## The invite is single-use and short-lived, and it goes through instance metadata
#
# `install.sh <token>` pairs the node with *this* Mac, so the token is minted here and handed over
# in metadata. Metadata is readable by anyone with project access, which for a bearer token would
# normally be a problem — it is not one here because the token is spent within a minute of first
# boot and dies on its own an hour later (`--ttl`), and because spending it a second time is
# refused by construction (`join.rs`, the invite book). It is removed from metadata after the pair.
#
# Pairing with this Mac is only about *managing* the box. It is not what App Review uses: for that
# the demo node MINTS and the reviewer's phone spends. That is the other direction, and the last
# thing this script prints.
set -euo pipefail

NAME="adi-demo"
PROJECT="mono-504617"
ZONE="europe-southwest1-a"     # next to adi-relay-mad, so the demo's fallback path is a short one
MACHINE_TYPE="e2-small"        # not e2-micro: a node runs the panel, the hive and DNS, not one proxy
DISK_GB=20
IMAGE_FAMILY="debian-12"
IMAGE_PROJECT="debian-cloud"
NODE_USER="adi"
ACCOUNT=""
INVITE=""
INVITE_TTL=60                  # minutes; it only has to survive one boot
DO_PAIR=1
DRY_RUN=0
PRINT_STARTUP=0

die() { echo "error: $*" >&2; exit 1; }
# Progress goes to stderr so that stdout carries only the thing being asked for — which is what
# makes `--print-startup | bash -n` check the startup script instead of choking on a banner.
say() { echo "$@" >&2; }
usage() {
  awk 'NR>1 { if ($0 !~ /^#/) exit; sub(/^# ?/, ""); print }' "${BASH_SOURCE[0]}"
  echo
  echo "Options: --name N  --project P  --zone Z  --machine-type T  --disk-gb N  --account A"
  echo "         --invite TOKEN  --invite-ttl MIN  --no-pair  --dry-run  --print-startup"
  exit "${1:-0}"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --name) NAME="$2"; shift 2 ;;
    --project) PROJECT="$2"; shift 2 ;;
    --zone) ZONE="$2"; shift 2 ;;
    --machine-type) MACHINE_TYPE="$2"; shift 2 ;;
    --disk-gb) DISK_GB="$2"; shift 2 ;;
    --account) ACCOUNT="$2"; shift 2 ;;
    --invite) INVITE="$2"; shift 2 ;;
    --invite-ttl) INVITE_TTL="$2"; shift 2 ;;
    --no-pair) DO_PAIR=0; shift ;;
    --dry-run) DRY_RUN=1; shift ;;
    # The startup script is a heredoc inside a heredoc's worth of escaping, and `bash -n` on this
    # file does not check a word of it. This is how you look at what the VM will actually run —
    # and how you syntax-check it:  scripts/deploy-demo-node.sh --print-startup | bash -n
    # Implies --dry-run: printing what the VM would run must never need a live credential.
    --print-startup) PRINT_STARTUP=1; DRY_RUN=1; shift ;;
    -h|--help) usage 0 ;;
    *) die "unknown argument: $1" ;;
  esac
done

[[ "$NAME" =~ ^[a-z]([a-z0-9-]{0,61}[a-z0-9])?$ ]] || die "not a valid GCE instance name: $NAME"
[[ "$ZONE" =~ ^[a-z0-9-]+-[a-z]$ ]] || die "zone looks wrong (want e.g. us-east4-a): $ZONE"

command -v gcloud >/dev/null || die "gcloud is not installed"

gc=(gcloud --project="$PROJECT" --quiet)
[[ -n "$ACCOUNT" ]] && gc+=(--account="$ACCOUNT")

run() {
  if [[ $DRY_RUN -eq 1 ]]; then printf '  + %q' "$1"; printf ' %q' "${@:2}"; printf '\n'; return 0; fi
  "$@"
}

say "==> demo node $NAME  →  project $PROJECT, zone $ZONE, $MACHINE_TYPE, ${DISK_GB}GB, $IMAGE_FAMILY"

# Snapshotted before anything is created, so §4 can tell which fleet entry is the new box.
FLEET_BEFORE="$(adi-mono mesh fleet 2>/dev/null | grep -oE '^[a-z0-9][a-z0-9._-]*' | sort || true)"

if [[ $DRY_RUN -eq 0 ]]; then
  "${gc[@]}" projects describe "$PROJECT" >/dev/null 2>&1 || die \
"cannot reach project $PROJECT.

  The credential on this machine is stale — every compute call fails with
  'Reauthentication failed. cannot prompt during non-interactive execution'.
  It needs a browser, so it cannot be done from a script:

      gcloud auth login --account=ihor@withadi.dev

  (mgorunuch.igor@gmail.com is also authenticated but has no compute permission
  on this project, so switching accounts is not the fix.)"
fi

# ---------------------------------------------------------------------------- 1. the invite
# Minted here, on the machine the node will be paired *to*. Skipped entirely with --no-pair, which
# leaves a node that is up but reachable by nobody until you run `adi-mono mesh join` on it.
if [[ $DO_PAIR -eq 1 && -z "$INVITE" ]]; then
  say "==> minting an invite on this machine (ttl ${INVITE_TTL}m)"
  if [[ $DRY_RUN -eq 1 ]]; then
    INVITE="adi-invite:<minted-at-run-time>"
    say "    (dry run — would mint one)"
  else
    INVITE="$(adi-mono mesh invite --ttl "$INVITE_TTL" --no-qr --json \
      | python3 -c 'import json,sys; print(json.load(sys.stdin)["token"])')" \
      || die "could not mint an invite — is the mesh running here? (adi-mono status)"
    say "    minted (${#INVITE} chars)"
  fi
fi

# ---------------------------------------------------------------------------- 2. the startup script
# Runs as root on every boot, so it provisions once and exits early afterwards.
startup="$(cat <<EOF
#!/usr/bin/env bash
set -euo pipefail
exec >>/var/log/adi-provision.log 2>&1
echo "=== adi provision \$(date -Is) ==="

# A startup script runs on every boot, not just the first.
if [[ -e /var/lib/adi-provisioned ]]; then echo "already provisioned"; exit 0; fi

# The node runs unprivileged under \`systemd --user\`. Lingering is what keeps those services alive
# with nobody logged in, and it is the one part of this that does need root.
id -u "$NODE_USER" >/dev/null 2>&1 || useradd --create-home --shell /bin/bash "$NODE_USER"
loginctl enable-linger "$NODE_USER"

apt-get update -qq
# \`lsof\` is not optional, however much it looks it: it is how adi-app finds out what is listening
# (\`adi-app/src/scan.rs\`), and that scan is the *only* source of a service's running flag. A
# Debian image ships \`ss\` and not \`lsof\`, so without this line every service on the node reports
# itself stopped while serving perfectly well — the panel says the fleet is dead, the phone repeats
# it, and nothing is actually wrong. This node cost a day to that on 2026-09-03.
apt-get install -y -qq curl tar ca-certificates lsof

uid="\$(id -u "$NODE_USER")"
# \`systemd --user\` is reached over the user's own bus; without both of these, \`adi-mono up\`
# cannot see a manager to install its units into and the install half-succeeds.
export XDG_RUNTIME_DIR="/run/user/\$uid"
export DBUS_SESSION_BUS_ADDRESS="unix:path=\$XDG_RUNTIME_DIR/bus"
for _ in \$(seq 1 30); do [[ -d "\$XDG_RUNTIME_DIR" ]] && break; sleep 1; done

work="/home/$NODE_USER/install"
install -d -o "$NODE_USER" -g "$NODE_USER" "\$work"
curl -fsSL --retry 5 --retry-delay 3 \
  https://github.com/adi-family/mono/releases/latest/download/adi-linux-x64.tar.gz \
  -o "\$work/adi.tar.gz"
tar -xzf "\$work/adi.tar.gz" -C "\$work"
chown -R "$NODE_USER:$NODE_USER" "\$work"

pkg="\$(find "\$work" -maxdepth 2 -name install.sh -type f | head -1)"
[[ -n "\$pkg" ]] || { echo "no install.sh in the tarball"; exit 1; }

# The token is read from metadata rather than baked in here, so it is removed from one place.
token="\$(curl -fsS -H 'Metadata-Flavor: Google' \
  'http://metadata.google.internal/computeMetadata/v1/instance/attributes/adi-invite' || true)"

cd "\$(dirname "\$pkg")"

# HOME is spelled out because \`runuser -u\` does NOT set it — it keeps the *caller's* environment,
# and the caller here is root. Without this, install.sh lays the node out under /root/.local/adi
# while every service runs as $NODE_USER, and the box comes up with nothing on it. \`runuser -l\`
# would fix HOME and drop XDG_RUNTIME_DIR instead, which breaks \`systemd --user\` the other way.
as_node() {
  runuser -u "$NODE_USER" -- env \\
    HOME="/home/$NODE_USER" USER="$NODE_USER" LOGNAME="$NODE_USER" \\
    XDG_RUNTIME_DIR="\$XDG_RUNTIME_DIR" \\
    DBUS_SESSION_BUS_ADDRESS="\$DBUS_SESSION_BUS_ADDRESS" \\
    "\$@"
}

if [[ -n "\$token" ]]; then
  as_node ./install.sh "\$token"
else
  as_node ./install.sh --no-pair
fi

touch /var/lib/adi-provisioned
echo "=== done \$(date -Is) ==="
EOF
)"

if [[ $PRINT_STARTUP -eq 1 ]]; then printf '%s\n' "$startup"; exit 0; fi

# ---------------------------------------------------------------------------- 3. the VM
say "==> instance $NAME"
if "${gc[@]}" compute instances describe "$NAME" --zone="$ZONE" >/dev/null 2>&1; then
  say "    already exists — leaving it alone (delete it first to rebuild)"
else
  meta_file="$(mktemp)"; printf '%s' "$startup" >"$meta_file"
  trap 'rm -f "$meta_file"' EXIT
  args=(compute instances create "$NAME"
        --zone="$ZONE" --machine-type="$MACHINE_TYPE"
        --image-family="$IMAGE_FAMILY" --image-project="$IMAGE_PROJECT"
        --boot-disk-size="${DISK_GB}GB" --boot-disk-type=pd-balanced
        --metadata-from-file=startup-script="$meta_file"
        --labels=role=demo-node,managed-by=deploy-demo-node
        --description="adi demo node — a fleet a stranger can be shown; see apps/ios/APPSTORE.md")
  [[ -n "$INVITE" ]] && args+=(--metadata=adi-invite="$INVITE")
  run "${gc[@]}" "${args[@]}"
fi

if [[ $DRY_RUN -eq 1 ]]; then echo; echo "(dry run — nothing was created)"; exit 0; fi

# ---------------------------------------------------------------------------- 4. wait, then tidy
# The only honest signal that this worked is the node turning up in *this* machine's fleet: the VM
# reaching RUNNING says nothing (the startup script has barely begun) and so does a metadata flag we
# would be setting ourselves. So diff the fleet against what it was before the VM existed.
fleet_petnames() { adi-mono mesh fleet 2>/dev/null | grep -oE '^[a-z0-9][a-z0-9._-]*' | sort; }

if [[ $DO_PAIR -eq 1 ]]; then
  printf '%s' "==> waiting for the node to provision and pair (fetches ~50MB first)"
  new=""
  for _ in $(seq 1 90); do
    sleep 10
    new="$(comm -13 <(printf '%s\n' "$FLEET_BEFORE") <(fleet_petnames) | head -1)"
    [[ -n "$new" ]] && break
    echo -n "."
  done
  echo
  if [[ -n "$new" ]]; then
    say "    paired as: $new"
  else
    say "    not paired within 15 minutes — it may still be installing. See the log command below."
  fi
fi

# The token is spent by now, and a spent bearer token in metadata is still a bearer token in
# metadata. Remove it whether or not the pairing succeeded — a failed one is re-driven by
# re-running this script, which mints a fresh token anyway.
if [[ -n "$INVITE" ]]; then
  say "==> removing the invite from instance metadata"
  run "${gc[@]}" compute instances remove-metadata "$NAME" --zone="$ZONE" --keys=adi-invite || true
fi

cat <<EOF

==> the box is up. Two things are left, and neither can be done from here.

1. CHECK THE PAIR, and read the provisioning log if it did not land:

     adi-mono mesh fleet                          # the node should be listed
     gcloud compute ssh $NAME --zone=$ZONE --project=$PROJECT \\
       --command 'sudo tail -40 /var/log/adi-provision.log'

   Once it is paired, its control panel is at  http://app.<petname>.n.adi/  from this Mac.

2. MINT THE REVIEW INVITES — ON THE NODE, not here. This is the other direction: the node mints
   and the phone spends, which is what lets a reviewer pair with a machine they will never touch.

     gcloud compute ssh $NAME --zone=$ZONE --project=$PROJECT \\
       --command 'for i in 1 2 3 4; do adi-mono mesh invite --ttl 20160 --no-qr --json; done'

   Fourteen days each. Put ALL FOUR in App Review Information → Notes: an invite is one machine
   once, so a reviewer who retries, or a second reviewer on an appeal, needs the next one.
   The note to paste is drafted in apps/ios/APPSTORE.md §2.2.

   Whatever you want the reviewer to SEE has to exist on this node — its dashboards are what the
   app lists. That is also the fleet the App Store screenshots would be taken against, so it is
   worth making it look like something.

EOF
