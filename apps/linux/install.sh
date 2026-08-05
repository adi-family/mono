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
Usage: ./install.sh [--prefix DIR] [--no-pair] [--no-bun] <invite-token>

  --prefix DIR   where to install the binaries (default: ~/.local/adi)
  --no-pair      install and start the services, but do not pair yet
  --no-bun       skip fetching the bun runtime (dashboards will not run)
  -h, --help     this message

The invite token comes from the machine you are pairing with; it is a single opaque
string, so quote it if your shell might touch it.
USAGE
}

# The bun runtime a dashboard's two servers are run by. Pinned by version *and* checksum, and
# fetched from oven-sh at install time rather than shipped in this tarball: bun is MIT, but it
# statically links JavaScriptCore (LGPL-2) and tinycc (LGPL-2.1), and redistributing the binary
# inside our package would carry their relink obligation with it. Downloading it here means the
# operator gets it from upstream, unmodified, exactly as if they had run bun's own installer —
# and we still get to pin the version the node runs.
BUN_VERSION="1.3.14"
BUN_SHA256_MODERN="951ee2aee855f08595aeec6225226a298d3fea83a3dcd6465c09cbccdf7e848f"
BUN_SHA256_BASELINE="a063908ae08b7852ca10939bbdc6ceed3ddabce8fb9402dce83d65d73b36e6c7"

PREFIX="${ADI_PREFIX:-$HOME/.local/adi}"
TOKEN=""
PAIR=1
WANT_BUN=1

while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)   [ $# -ge 2 ] || die "--prefix needs a directory"; PREFIX="$2"; shift 2 ;;
        --prefix=*) PREFIX="${1#--prefix=}"; shift ;;
        --no-pair)  PAIR=0; shift ;;
        --no-bun)   WANT_BUN=0; shift ;;
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

# ── 1b. bun ─────────────────────────────────────────────────────────────────────────────────
# Into $PREFIX/bin deliberately, not ~/.bun/bin: that directory is on the PATH adi-core writes
# into every `systemd --user` unit, so the dashboards supervisor can actually resolve `bun run …`.
# bun's own installer lands it somewhere the units do not look.

sha256_of() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1
    elif command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | cut -d' ' -f1
    elif command -v openssl >/dev/null 2>&1; then openssl dgst -sha256 "$1" | awk '{print $NF}'
    else return 1
    fi
}

# bun publishes two x64 builds: the default needs AVX2, `-baseline` does not. Picking wrong gives
# an "Illegal instruction" crash at first run rather than a clear failure here, so ask the CPU.
bun_variant() {
    if grep -qm1 ' avx2 ' /proc/cpuinfo 2>/dev/null; then echo "bun-linux-x64"
    else echo "bun-linux-x64-baseline"
    fi
}

# bun ships only as a .zip, and a minimal cloud image frequently has no `unzip` — that is exactly
# where bun's own installer stops with "unzip is required". python3 is far more likely to be there,
# and its stdlib reads zips, so it is the fallback rather than a second package to install.
extract_bun() {
    _zip="$1"; _dest_dir="$2"
    if command -v unzip >/dev/null 2>&1; then
        unzip -q -o -j "$_zip" '*/bun' -d "$_dest_dir" 2>/dev/null && return 0
    fi
    if command -v python3 >/dev/null 2>&1; then
        python3 - "$_zip" "$_dest_dir" <<'PY' && return 0
import sys, zipfile, pathlib
zip_path, dest = sys.argv[1], pathlib.Path(sys.argv[2])
with zipfile.ZipFile(zip_path) as z:
    name = next((n for n in z.namelist() if n.rsplit("/", 1)[-1] == "bun"), None)
    if name is None:
        sys.exit("no bun executable inside the archive")
    (dest / "bun").write_bytes(z.read(name))
PY
    fi
    return 1
}

install_bun() {
    # Already satisfied? Prefer our own copy, then anything the operator put on PATH.
    if [ -x "$PREFIX/bin/bun" ]; then
        say "bun already installed ($("$PREFIX/bin/bun" --version 2>/dev/null || echo "unknown"))"
        return 0
    fi
    if command -v bun >/dev/null 2>&1; then
        say "bun already on PATH at $(command -v bun) ($(bun --version 2>/dev/null || echo "unknown"))"
        return 0
    fi

    command -v curl >/dev/null 2>&1 || { echo "warning: no curl; cannot fetch bun" >&2; return 1; }

    _variant="$(bun_variant)"
    case "$_variant" in
        bun-linux-x64) _want="$BUN_SHA256_MODERN" ;;
        *)             _want="$BUN_SHA256_BASELINE" ;;
    esac
    _url="https://github.com/oven-sh/bun/releases/download/bun-v$BUN_VERSION/$_variant.zip"

    _tmp="$(mktemp -d)" || return 1
    say "fetching bun $BUN_VERSION ($_variant)"
    if ! curl -fsSL --retry 3 -o "$_tmp/bun.zip" "$_url"; then
        echo "warning: could not download $_url" >&2
        rm -rf "$_tmp"; return 1
    fi

    # Verified before it is ever made executable: this binary runs every dashboard on the node.
    _got="$(sha256_of "$_tmp/bun.zip")" || {
        echo "warning: no sha256 tool (sha256sum/shasum/openssl); refusing to install bun unverified" >&2
        rm -rf "$_tmp"; return 1
    }
    if [ "$_got" != "$_want" ]; then
        echo "warning: bun checksum mismatch for $_variant" >&2
        echo "         expected $_want" >&2
        echo "         got      $_got" >&2
        rm -rf "$_tmp"; return 1
    fi

    if ! extract_bun "$_tmp/bun.zip" "$_tmp"; then
        echo "warning: could not unpack bun (needs unzip or python3)" >&2
        rm -rf "$_tmp"; return 1
    fi
    [ -f "$_tmp/bun" ] || { echo "warning: no bun in the archive" >&2; rm -rf "$_tmp"; return 1; }

    rm -f "$PREFIX/bin/bun"
    mv "$_tmp/bun" "$PREFIX/bin/bun"
    chmod 0755 "$PREFIX/bin/bun"
    rm -rf "$_tmp"
    say "bun $("$PREFIX/bin/bun" --version 2>/dev/null || echo "$BUN_VERSION") installed into $PREFIX/bin"
}

# Never fatal: the control panel, the mesh and project services all run without bun. Only
# dashboards need it, so a node with no outbound access to GitHub is still a working node.
if [ "$WANT_BUN" -eq 1 ]; then
    install_bun || echo "warning: continuing without bun — dashboards on this node will not run" >&2
else
    say "skipping bun (--no-bun); dashboards on this node will not run"
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

# Dashboards are a pair of bun servers. Step 1b normally has it by now; say so plainly when it
# does not, rather than let the operator discover it as a dashboard that is listed, has a
# hostname, and never answers — the supervisor starts and finds nothing to run, which looks like
# a broken dashboard rather than a missing dependency. Everything else on the node works without.
if [ ! -x "$PREFIX/bin/bun" ] && ! command -v bun >/dev/null 2>&1; then
    say "note: bun is not installed, so dashboards on this node cannot run."
    say "      re-run this installer, or:  curl -fsSL https://bun.sh/install | bash"
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
