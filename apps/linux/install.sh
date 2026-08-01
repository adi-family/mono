#!/bin/sh
#
# Install this ADI node package and pair it with a fleet.
#
#   ./install.sh <invite-token>
#   ./install.sh --prefix /opt/adi <invite-token>
#   ./install.sh --no-pair                 # install only; pair later with `adi-mono mesh join`
#
# What it does, in order:
#
#   1. copies the five binaries into $PREFIX/bin (default ~/.local/adi/bin) and puts that
#      directory on PATH, because `adi-mono` is re-invoked *by name* by the agent harness;
#   2. enables logind *lingering*, so the per-user systemd manager — and every adi service under
#      it — keeps running after the installing session logs out. A node is headless; without
#      this the whole stack dies the moment you close the ssh session that installed it;
#   3. runs `adi-mono up`, which writes one `systemd --user` unit per service into
#      ~/.config/systemd/user and enables it (adi-core's supervisor abstraction owns the unit
#      contents — this script never templates a unit itself);
#   4. pairs, with `adi-mono mesh join <token>`.
#
# It needs **no root** and opens **no inbound port**, not even during install: the node dials
# out to an iroh relay and is reached back down that session. See README.md.
#
# POSIX sh on purpose — a minimal node image may have no bash.
set -eu

die() { echo "error: $*" >&2; exit 1; }
say() { echo "==> $*"; }

usage() {
    cat <<'USAGE'
Usage: ./install.sh [--prefix DIR] [--no-pair] <invite-token>

  --prefix DIR   where to install the binaries (default: ~/.local/adi)
  --no-pair      install and start the services, but do not pair yet
  -h, --help     this message

The invite token comes from the machine you are pairing with; it is a single opaque
string, so quote it if your shell might touch it.
USAGE
}

PREFIX="${ADI_PREFIX:-$HOME/.local/adi}"
TOKEN=""
PAIR=1

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)   [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
        --no-pair)  PAIR=0; shift ;;
        -h|--help)  usage; exit 0 ;;
        --)         shift; if [ $# -gt 0 ]; then TOKEN="$1"; fi; break ;;
        -*)         die "unknown option: $1 (see --help)" ;;
        *)          [ -z "$TOKEN" ] || die "more than one invite token given"; TOKEN="$1"; shift ;;
    esac
done

HERE="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
[ -d "$HERE/bin" ] || die "no bin/ beside $0 — run install.sh from inside the unpacked package"

[ "$PAIR" -eq 0 ] || [ -n "$TOKEN" ] || {
    usage >&2
    die "an invite token is required (or pass --no-pair to install without pairing)"
}

# ── preflight ───────────────────────────────────────────────────────────────────────────────
command -v systemctl >/dev/null 2>&1 \
    || die "systemctl not found — this package supervises its services with systemd --user"

# `systemctl --user` reaches the per-user manager over the bus under XDG_RUNTIME_DIR. A login
# shell has it; `ssh node ./install.sh` without a tty may not, and then every systemctl call
# fails with "Failed to connect to bus" no matter how healthy the manager is.
if [ -z "${XDG_RUNTIME_DIR:-}" ]; then
    XDG_RUNTIME_DIR="/run/user/$(id -u)"
    export XDG_RUNTIME_DIR
    say "XDG_RUNTIME_DIR was unset; using $XDG_RUNTIME_DIR"
fi

# ── 1. binaries ─────────────────────────────────────────────────────────────────────────────
say "installing binaries into $PREFIX/bin"
mkdir -p "$PREFIX/bin"
for b in adi-mono adi-dns adi-hive adi-app adi-mesh; do
    [ -f "$HERE/bin/$b" ] || die "missing $HERE/bin/$b — incomplete package"
    # Remove first: copying over a *running* executable fails with ETXTBSY, which is exactly
    # what an upgrade-in-place is. Unlinking leaves the running process on the old inode.
    rm -f "$PREFIX/bin/$b"
    cp "$HERE/bin/$b" "$PREFIX/bin/$b"
    chmod 0755 "$PREFIX/bin/$b"
done

# Record the installed version beside the binaries. A node has no app bundle and so no
# Info.plist, and this file is what `adi-mono update` reads to decide whether a published
# release is newer than what is here. Without it the updater falls back to the version
# compiled into whichever binary happens to be running, which drifts after the first update.
cp "$HERE/VERSION" "$PREFIX/bin/VERSION" 2>/dev/null \
    || die "missing $HERE/VERSION — incomplete package"

PATH="$PREFIX/bin:$PATH"
export PATH

PROFILE="$HOME/.profile"
if ! grep -qF "$PREFIX/bin" "$PROFILE" 2>/dev/null; then
    say "adding $PREFIX/bin to PATH in $PROFILE"
    printf '\n# added by the ADI node installer\nexport PATH="%s/bin:$PATH"\n' "$PREFIX" >> "$PROFILE"
fi

# ── 2. lingering ────────────────────────────────────────────────────────────────────────────
# `adi-mono up` asks for this too (adi-core's systemd back-end does it on every enable); doing
# it here as well means the node is already durable before any unit is written, and that a
# refusal is reported at the top of the install rather than buried in service output.
USER_NAME="$(id -un)"
if command -v loginctl >/dev/null 2>&1; then
    if [ "$(loginctl show-user "$USER_NAME" --property=Linger --value 2>/dev/null || echo no)" = "yes" ]; then
        say "lingering already enabled for $USER_NAME"
    elif loginctl enable-linger "$USER_NAME" >/dev/null 2>&1; then
        say "lingering enabled for $USER_NAME"
    else
        echo "warning: could not enable lingering for $USER_NAME" >&2
        echo "         run: sudo loginctl enable-linger $USER_NAME" >&2
        echo "         without it every adi service stops when $USER_NAME logs out" >&2
    fi
else
    echo "warning: loginctl not found; cannot enable lingering — services may stop at logout" >&2
fi

# ── 3. services ─────────────────────────────────────────────────────────────────────────────
say "starting services (adi-mono up)"
adi-mono up

# ── 4. pairing ──────────────────────────────────────────────────────────────────────────────
if [ "$PAIR" -eq 1 ]; then
    say "pairing this node"
    adi-mono mesh join "$TOKEN"
fi

echo
say "done — $(cat "$HERE/VERSION" 2>/dev/null || echo "adi") installed in $PREFIX"

# Dashboards are a pair of bun servers, and bun is not ours to ship — it is a separate runtime
# with its own release cadence and licence. Say so once, here, rather than let the operator
# discover it as a dashboard that is listed, has a hostname, and never answers: the supervisor
# starts and finds nothing to run, which looks like a broken dashboard rather than a missing
# dependency. Everything else on the node works without it.
if ! command -v bun >/dev/null 2>&1; then
    say "note: bun is not installed, so dashboards on this node cannot run."
    say "      install it with:  curl -fsSL https://bun.sh/install | bash"
    say "      the rest of the node — control panel, mesh, services — needs nothing further."
fi

adi-mono status || true
cat <<EOF

Next:
  * open a new shell (or: export PATH="$PREFIX/bin:\$PATH") so \`adi-mono\` is on PATH
  * adi-mono status                              what is enabled and running
  * systemctl --user list-units 'family.adi.*'   the units behind that
  * tail -f ~/.adi/mono/logs/*.log               service output (the units append to files,
                                                 so this is the log, not journalctl)
  * README.md                                    firewall, DNS, and how to reach this node
EOF
