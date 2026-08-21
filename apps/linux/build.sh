#!/usr/bin/env bash
#
# Build the Linux ADI **node** package: cross-compile the five node binaries for
# x86_64-unknown-linux-musl, assemble them with an installer + README, and tar it.
#
# musl, not glibc: each binary links statically, so one tarball runs on Debian, RHEL, Alpine or
# anything else without having to match the distro's libc. A fleet node is whatever cheap box was
# available; the package must not care which.
#
# There is no launcher here, unlike the Windows package. A node is headless (docs/fleet.md §6):
# no GUI, no browser to open, and nothing listening off loopback. The entry point is install.sh —
# it lands the binaries, brings the services up under `systemd --user`, enables lingering so they
# survive logout, and pairs the node with an invite token.
#
# Output:  apps/linux/build/adi-linux-x64/         (the unpacked package)
#          apps/linux/build/adi-linux-x64.tar.gz   (the shippable archive)
#
# Requirements (on the build host) — two supported routes, picked automatically:
#
#   BUILDER=cargo   (the default: a plain cross-compile, and the fast one)
#       rustup target add x86_64-unknown-linux-musl
#       a musl cross toolchain with **both** a C and a C++ compiler — `x86_64-linux-musl-gcc`
#       and `x86_64-linux-musl-g++`. The C++ half is not optional: usearch, reached through
#       adi-indexer, is C++.
#           macOS:         brew install FiloSottile/musl-cross/musl-cross   (ships both, GCC 14)
#           Debian/Ubuntu: `musl-tools` is C only, and there is no musl C++ in the archive —
#                          build in a musl-native container instead (below).
#
#   a musl-native host   (no flag: the script notices, and there is nothing to cross-compile)
#     Alpine's own libc is musl, so `cc`/`c++` already target it and the toolchain question
#     disappears. This is how CI builds the package:
#         docker run --rm -v "$PWD:/w" -w /w alpine:3.22 sh -c \
#           'apk add build-base curl bash git tar file && …rustup… && ./apps/linux/build.sh'
#     It is also the only easy way to get a musl toolchain new enough: the prebuilt
#     musl-cross-make tarballs on musl.cc stop at GCC 11, too old for usearch's SIMD backend
#     (numkong), and zig's clang fails on the same code. Alpine 3.22 is GCC 14.
#
#   BUILDER=cross   (a container build; needs `cargo install cross` + Docker/Podman)
#     Kept as the escape hatch for a host with no musl toolchain. It used to be the *recommended*
#     route for a second reason that no longer exists: the tree pulled OpenSSL through
#     reqwest → native-tls, and only cross's image carried an OpenSSL built for musl. That
#     dependency is gone — every reqwest in the workspace now declares
#     `default-features = false` with rustls — so a static musl binary needs no C crypto at all
#     and the plain cargo route works on any host with the linker.
#
# Set SKIP_BUILD=1 to only re-assemble from an existing target/ (useful when iterating on
# install.sh or the README).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

TARGET="x86_64-unknown-linux-musl"
PKG_NAME="adi-linux-x64"
BUILD="$SCRIPT_DIR/build"
PKG="$BUILD/$PKG_NAME"

# docs/fleet.md §6: a node runs the CLI, the resolver, the front door, the control panel and the
# mesh. The mesh *daemon* runs in-process inside adi-app (that is what its "Start mesh" button
# starts), so `adi-mesh` ships for the standalone and debugging cases — `adi-mesh id`, `ticket`,
# `forward` — rather than because a node cannot come up without it.
BINS=(adi-mono adi-dns adi-hive adi-app adi-mesh)
CRATES=(-p adi-cli -p adi-dns -p adi-hive -p adi-app -p adi-mesh)

# The git tag is the source of truth (scripts/version.sh), same as the macOS and Windows builds.
# Exported so the binaries compile it in as `BUILT_VERSION` — the node's updater compares that
# against the published manifest, and it must match the VERSION file this package ships.
VERSION="$("$ROOT/scripts/version.sh")"
export ADI_VERSION="$VERSION"

# adi-app embeds the webapp at compile time and happily embeds nothing — see the script.
"$ROOT/scripts/require-webapp-dist.sh"

have_target() {
    rustup target list --installed 2>/dev/null | grep -qx "$TARGET"
}

# Whether this host's own libc is already musl — an Alpine container, or a musl-native distro.
# There the "cross" build is a *native* one: `cc`/`c++` already target musl, there is no
# `x86_64-linux-musl-` prefix to find, and cargo needs no linker override.
#
# This is not an exotic case, it is how CI builds this package. A musl cross toolchain new
# enough for the tree is otherwise hard to come by: usearch's SIMD backend (numkong, reached
# through adi-indexer) needs a compiler that understands the recent x86 target attributes, and
# the prebuilt musl-cross-make tarballs on musl.cc stop at GCC 11, which does not. Alpine ships
# GCC 14 with musl as its own libc, so the problem disappears instead of being worked around.
host_is_musl() {
    rustc -vV 2>/dev/null | grep -q "^host: .*-linux-musl$"
}

# The musl C **and C++** compilers. Sets MUSL_CC/MUSL_CXX as a side effect, so the preflight can
# name what is missing and the builder can hand both to cargo.
#
# C++ is not optional any more and used to be unasked for: `usearch` is C++, so a host with
# `musl-gcc` and no `musl-g++` passed every check here and then failed minutes later inside
# cc-rs, naming a compiler this script had never looked for.
have_musl_cc() {
    if host_is_musl; then
        MUSL_CC="cc"
        MUSL_CXX="c++"
        return 0
    fi
    for candidate in x86_64-linux-musl-gcc musl-gcc; do
        if command -v "$candidate" >/dev/null 2>&1; then
            MUSL_CC="$candidate"
            # `musl-gcc` is a specs wrapper around the host gcc and has no C++ twin; the
            # musl-cross-make toolchains ship the `-g++` beside the `-gcc`.
            MUSL_CXX="$(printf '%s' "$candidate" | sed 's/gcc$/g++/')"
            command -v "$MUSL_CXX" >/dev/null 2>&1 || MUSL_CXX=""
            return 0
        fi
    done
    return 1
}

# ── pick a builder ──────────────────────────────────────────────────────────────────────────
# Plain cargo by default now that nothing in the tree needs a C crypto library: it is a native
# compile with a cross-linker, so it is minutes faster than spinning a container, and it works on
# any host that has the musl toolchain. `cross` stays available for hosts that don't.
#
# These two probes are defined *above* this block on purpose: sh resolves a function only once it
# has been read, so with the definitions below it `have_musl_cc` was an unknown command here, the
# `if` took its non-zero exit as "no toolchain", and auto-detection could never choose `cargo` —
# every build silently fell through to the container route, on hosts that had the linker installed.
BUILDER="${BUILDER:-auto}"
if [ "$BUILDER" = "auto" ]; then
    if have_musl_cc; then
        BUILDER="cargo"
    elif command -v cross >/dev/null 2>&1 && docker info >/dev/null 2>&1; then
        BUILDER="cross"
    else
        BUILDER="cargo"   # fails in preflight with the exact thing to install
    fi
fi

# Fail *before* spending minutes compiling, and name the exact thing that is missing — a build
# host is usually missing one of three specific pieces, and "linker not found" three minutes in
# does not say which.
preflight_cargo() {
    have_target || {
        echo "error: rust target $TARGET is not installed" >&2
        echo "       run: rustup target add $TARGET" >&2
        exit 1
    }

    MUSL_CC=""
    MUSL_CXX=""
    have_musl_cc || {
        echo "error: no musl cross toolchain on PATH (looked for x86_64-linux-musl-gcc, musl-gcc)" >&2
        echo "       macOS:         brew install FiloSottile/musl-cross/musl-cross" >&2
        echo "       Debian/Ubuntu: apt install musl-tools" >&2
        echo "       or install \`cross\` (cargo install cross) and re-run with BUILDER=cross" >&2
        exit 1
    }
    [ -n "$MUSL_CXX" ] || {
        echo "error: found $MUSL_CC but no matching musl C++ compiler" >&2
        echo "       usearch (via adi-indexer) is C++, so this build needs one." >&2
        echo "       macOS:         brew install FiloSottile/musl-cross/musl-cross  (ships both)" >&2
        echo "       Debian/Ubuntu: musl-tools is C only — build in a musl-native container" >&2
        echo "                      instead: docker run --rm -v \"\$PWD:/w\" -w /w alpine:3.22 …" >&2
        echo "       (see the release workflow's linux job, which does exactly that)" >&2
        exit 1
    }
    # No OpenSSL check any more: every reqwest in the workspace declares
    # `default-features = false` with rustls, so openssl-sys is not in the graph at all. If that
    # ever regresses, the link fails here with an `openssl` symbol error — and the fix is the
    # Cargo.toml that reintroduced it, not an OpenSSL for musl.
}

preflight_cross() {
    command -v cross >/dev/null 2>&1 || {
        echo "error: BUILDER=cross but \`cross\` is not on PATH — run: cargo install cross" >&2
        exit 1
    }
    docker info >/dev/null 2>&1 || {
        echo "error: BUILDER=cross needs a running container runtime (Docker or Podman)" >&2
        exit 1
    }
}

if [ "${SKIP_BUILD:-}" != "1" ]; then
    echo "==> cross-compiling for $TARGET (release, builder=$BUILDER): ${BINS[*]}"
    case "$BUILDER" in
        cross)
            preflight_cross
            ( cd "$ROOT" && cross build --release --target "$TARGET" "${CRATES[@]}" )
            ;;
        cargo)
            preflight_cargo
            # All three matter: cargo picks the linker, and cc-rs picks the compiler for the C
            # *and* the C++ in the dependency tree. Setting only some produces host objects in a
            # target link — or, for the C++ one, sends cc-rs looking for a compiler named after
            # the target that nobody installed.
            ( cd "$ROOT" \
                && CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="$MUSL_CC" \
                   CC_x86_64_unknown_linux_musl="$MUSL_CC" \
                   CXX_x86_64_unknown_linux_musl="$MUSL_CXX" \
                   cargo build --release --target "$TARGET" "${CRATES[@]}" )
            ;;
        *)
            echo "error: unknown BUILDER=$BUILDER (expected 'cross' or 'cargo')" >&2
            exit 1
            ;;
    esac
fi

# Verify every binary exists before packaging.
OUT="$ROOT/target/$TARGET/release"
for b in "${BINS[@]}"; do
    [ -f "$OUT/$b" ] || { echo "error: $OUT/$b missing (build failed?)" >&2; exit 1; }
done

# A dynamically linked binary here would defeat the whole point — it would run on the build
# host's distro and nowhere else. Warn rather than fail: `file` is not everywhere, and its
# wording varies.
if command -v file >/dev/null 2>&1; then
    for b in "${BINS[@]}"; do
        case "$(file -b "$OUT/$b")" in
            *"statically linked"*|*"static-pie linked"*) ;;
            *) echo "warning: $b is not statically linked — it will need the target's libc" >&2 ;;
        esac
    done
fi

echo "==> assembling $PKG  (version $VERSION)"
rm -rf "$PKG"
mkdir -p "$PKG/bin"
for b in "${BINS[@]}"; do
    cp "$OUT/$b" "$PKG/bin/$b"
    chmod 0755 "$PKG/bin/$b"
done
echo "$VERSION" > "$PKG/VERSION"

cp "$SCRIPT_DIR/install.sh" "$PKG/install.sh"
chmod 0755 "$PKG/install.sh"
cp "$SCRIPT_DIR/README.md" "$PKG/README.md"

echo "==> tarring"
rm -f "$BUILD/$PKG_NAME.tar.gz"
# Tar from the build dir so the archive unpacks into one named directory rather than over the
# operator's cwd.
( cd "$BUILD" && tar -czf "$PKG_NAME.tar.gz" "$PKG_NAME" )

echo
echo "==> done"
echo "    package: $PKG"
echo "    tarball: $BUILD/$PKG_NAME.tar.gz"
echo
echo "    Install on a node:"
echo "      tar -xzf $PKG_NAME.tar.gz && cd $PKG_NAME && ./install.sh <invite-token>"
