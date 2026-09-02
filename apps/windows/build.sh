#!/usr/bin/env bash
#
# Build the Windows ADI package: cross-compile the platform binaries and the launcher for
# x86_64-pc-windows-gnu, assemble them, and produce the two things a release ships.
#
#   apps/windows/build/ADI-Setup-x64.exe    the installer — what a person downloads
#   apps/windows/build/ADI-windows-x64.zip  the same files as an archive — what the auto-updater
#                                           downloads, and what an unattended install unpacks
#   apps/windows/build/ADI-windows-x64/     the staged package both are made from
#
# The package's shape is the point of the whole exercise. The platform is four executables, and
# a person should never have to know that: they go into `bin\`, and what is visible is ADI.exe —
# a tray app that starts the stack and opens the control panel, the Windows counterpart of
# ADI.app. The layout mirrors the Linux package (`bin/` + VERSION + README) so the auto-updater's
# one payload rule covers all three platforms.
#
# There is no native window on Windows and no plan for one: the control panel is a web UI, which
# ADI.exe opens in the browser.
#
# Requirements (on the build host):
#   rustup target add x86_64-pc-windows-gnu
#   the mingw-w64 cross toolchain (`x86_64-w64-mingw32-gcc` on PATH) — `brew install mingw-w64`
#   makensis (NSIS 3.x) — `brew install makensis`, or `apt-get install nsis`
#   zip
#
# Run from macOS/Linux; it cross-compiles. Set SKIP_BUILD=1 to re-assemble from an existing
# target/ (useful when iterating on the installer), or SKIP_INSTALLER=1 to produce only the zip.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

ICO="$SCRIPT_DIR/ADI.ico"

# The app icon, from the same master the Mac's is drawn from -- one artwork, two containers, so
# the two platforms cannot drift apart the way the fleet icon once did. macOS-only (sips), which
# is fine: the .ico is committed, and this is only run when the mark itself changes.
#
# The 90% inset is the difference between the two conventions: a macOS icon floats in a shadow
# gutter (icon-gen.swift insets it to ~80%), a Windows one fills more of its square.
if [ "${1:-}" = "--regen-icon" ]; then
    echo "==> regenerating ADI.ico from apps/macos/ADI.icns"
    command -v magick >/dev/null || { echo "error: ImageMagick (magick) not found" >&2; exit 1; }
    command -v sips >/dev/null || { echo "error: sips not found -- run this on macOS" >&2; exit 1; }
    TMP="$(mktemp -d)"
    trap 'rm -rf "$TMP"' EXIT
    sips -s format png "$ROOT/apps/macos/ADI.icns" --out "$TMP/master.png" >/dev/null
    magick "$TMP/master.png" -trim +repage -resize 922x922 \
        -gravity center -background none -extent 1024x1024 "$TMP/square.png"
    magick "$TMP/square.png" -define icon:auto-resize=256,48,32,16 "$ICO"
    echo "    wrote $ICO"
    exit 0
fi

# ── makensis, wherever it actually works ─────────────────────────────────────────────────────
#
# Ubuntu's `nsis` package is what the release job uses and it is fine. Homebrew's `makensis`
# 3.12 on Apple Silicon is not: it dies with an uncaught std::bad_alloc while loading its own
# stub, on any script at all, so on a developer's Mac the compiler is present and useless. Rather
# than make the installer un-buildable on the machine everything else here is built on, fall back
# to a container — the same move apps/linux/build.sh already makes for musl.
#
# NSIS itself is a cross-compiler: it produces the same Windows .exe wherever it runs, so which
# of the two did the work is not observable in the output.

# Can this makensis actually compile something? A `-VERSION` check would pass on the broken one.
makensis_works() {
    local probe
    probe="$(mktemp -d)"
    printf 'OutFile "%s/probe.exe"\nSection\nSectionEnd\n' "$probe" > "$probe/probe.nsi"
    local ok=1
    if "$1" -V1 "$probe/probe.nsi" >/dev/null 2>&1 && [ -f "$probe/probe.exe" ]; then
        ok=0
    fi
    rm -rf "$probe"
    return "$ok"
}

# Compile the installer. Takes the four-part version; reads $VERSION, $PKG, $BUILD, $SCRIPT_DIR.
run_makensis() {
    local quad="$1"
    local args=(
        -V2
        "-DVERSION=$VERSION"
        "-DVERSION_QUAD=$quad"
    )
    local makensis="${MAKENSIS:-makensis}"
    if [ "${MAKENSIS_DOCKER:-}" != "1" ] && command -v "$makensis" >/dev/null && makensis_works "$makensis"; then
        "$makensis" "${args[@]}" \
            "-DSOURCE_DIR=$PKG" \
            "-DOUTFILE=$BUILD/$SETUP_NAME" \
            "$SCRIPT_DIR/installer/adi.nsi"
        return
    fi
    command -v docker >/dev/null || {
        echo "error: no working makensis, and no docker to run one in." >&2
        echo "       Install NSIS 3.x (Debian/Ubuntu: apt-get install nsis), point MAKENSIS at a" >&2
        echo "       working binary, or re-run with SKIP_INSTALLER=1 to build only the zip." >&2
        exit 1
    }
    echo "    (no working local makensis -- compiling the installer in a container)"
    # The whole apps/windows tree is mounted, because the script reads the staged package, the
    # icon and path.ps1 by relative path.
    # Files land on the mounted volume owned by the container's root. On macOS, where this path
    # is actually taken, the file-sharing layer maps that back to the calling user.
    docker run --rm -v "$SCRIPT_DIR:/w" -w /w -e DEBIAN_FRONTEND=noninteractive \
        debian:stable-slim sh -euc '
        apt-get update >/dev/null 2>&1 && apt-get install -y --no-install-recommends nsis >/dev/null 2>&1
        makensis '"$(printf '%q ' "${args[@]}")"' \
            "-DSOURCE_DIR=/w/build/'"$PKG_NAME"'" \
            "-DOUTFILE=/w/build/'"$SETUP_NAME"'" \
            /w/installer/adi.nsi
    '
}

TARGET="x86_64-pc-windows-gnu"
PKG_NAME="ADI-windows-x64"
SETUP_NAME="ADI-Setup-x64.exe"
BUILD="$SCRIPT_DIR/build"
PKG="$BUILD/$PKG_NAME"

# The four platform binaries — a CLI and three supervised daemons — plus the launcher, which is
# the only one of the five a person ever names.
BINS=(adi-mono adi-dns adi-hive adi-app adi-launcher)
CRATES=(-p adi-cli -p adi-dns -p adi-hive -p adi-app -p adi-launcher)

# The git tag is the source of truth (scripts/version.sh), same as the macOS and Linux builds.
# Exported so the binaries compile it in as `BUILT_VERSION`, matching the VERSION file below.
VERSION="$("$ROOT/scripts/version.sh")"
export ADI_VERSION="$VERSION"

# adi-app embeds the webapp at compile time and happily embeds nothing — see the script.
"$ROOT/scripts/require-webapp-dist.sh"

if [ "${SKIP_BUILD:-}" != "1" ]; then
    command -v x86_64-w64-mingw32-gcc >/dev/null || {
        echo "error: x86_64-w64-mingw32-gcc not found — install the mingw-w64 cross toolchain" >&2
        echo "       (macOS: brew install mingw-w64)" >&2
        exit 1
    }
    # usearch (reached through adi-indexer) includes <Windows.h>; mingw ships `windows.h`. On a
    # case-insensitive filesystem — macOS, Windows — the two are the same file and nobody
    # notices. On Linux they are not, and the build dies three minutes in inside cc-rs with the
    # compiler's own message swallowed. Say it here instead, where it is actionable.
    for d in /usr/x86_64-w64-mingw32/include /usr/share/mingw-w64/include; do
        if [ -f "$d/windows.h" ] && [ ! -e "$d/Windows.h" ]; then
            echo "error: $d has windows.h but no Windows.h, and this filesystem is" >&2
            echo "       case-sensitive — usearch includes <Windows.h> and will not compile." >&2
            echo "       fix it once with:  sudo ln -s windows.h $d/Windows.h" >&2
            exit 1
        fi
    done
    # The win32 threading model's libgcc references `__mingwthr_key_dtor`, which nothing
    # defines. It only bites once C++ is in the link (usearch → link-cplusplus → -lstdc++),
    # and then it bites at the very last step, after every crate has compiled. Homebrew's
    # mingw is posix by default; Debian/Ubuntu's is not.
    case "$(x86_64-w64-mingw32-gcc -v 2>&1)" in
        *"Thread model: win32"*)
            echo "error: this mingw uses the win32 threading model, whose libgcc leaves" >&2
            echo "       __mingwthr_key_dtor undefined once C++ is linked in." >&2
            echo "       Debian/Ubuntu ships a posix variant — switch to it with:" >&2
            echo "         sudo update-alternatives --set x86_64-w64-mingw32-gcc \\" >&2
            echo "           /usr/bin/x86_64-w64-mingw32-gcc-posix" >&2
            echo "         sudo update-alternatives --set x86_64-w64-mingw32-g++ \\" >&2
            echo "           /usr/bin/x86_64-w64-mingw32-g++-posix" >&2
            exit 1
            ;;
    esac
    # The launcher's icon and version block are compiled by windres, which mingw supplies. Its
    # absence is only a warning inside the crate's build script, so say it here where the
    # consequence — shipping an app with a blank icon — is worth stopping for.
    command -v x86_64-w64-mingw32-windres >/dev/null || {
        echo "error: x86_64-w64-mingw32-windres not found — ADI.exe would ship without its icon" >&2
        exit 1
    }
    echo "==> cross-compiling for $TARGET (release): ${BINS[*]}"
    ( cd "$ROOT" && cargo build --release --target "$TARGET" "${CRATES[@]}" )
fi

# Verify every .exe exists before packaging.
OUT="$ROOT/target/$TARGET/release"
for b in "${BINS[@]}"; do
    [ -f "$OUT/$b.exe" ] || { echo "error: $OUT/$b.exe missing (build failed?)" >&2; exit 1; }
done

echo "==> assembling $PKG  (version $VERSION)"
rm -rf "$PKG"
mkdir -p "$PKG/bin"
for b in adi-mono adi-dns adi-hive adi-app; do
    cp "$OUT/$b.exe" "$PKG/bin/$b.exe"
done
# `adi-launcher` is the crate; `ADI` is the app. The name in the Start menu is the product's,
# not the build artifact's.
cp "$OUT/adi-launcher.exe" "$PKG/bin/ADI.exe"
# Beside the launcher as a fallback for its tray icon (it prefers its own embedded copy), and
# available to anything else that wants the artwork.
cp "$SCRIPT_DIR/ADI.ico" "$PKG/bin/ADI.ico"
echo "$VERSION" > "$PKG/VERSION"

# `adi` shim so the CLI answers to the platform's own name from any shell. In bin\ with the
# binaries, which is the directory the installer puts on PATH.
cat > "$PKG/bin/adi.cmd" <<'CMD'
@echo off
"%~dp0adi-mono.exe" %*
CMD

cp "$SCRIPT_DIR/README.md" "$PKG/README.txt"
cp "$ROOT/LICENSE" "$PKG/LICENSE.txt"

echo "==> zipping"
rm -f "$BUILD/$PKG_NAME.zip"
( cd "$BUILD" && zip -qr "$PKG_NAME.zip" "$PKG_NAME" )

if [ "${SKIP_INSTALLER:-}" = "1" ]; then
    echo "==> skipping the installer (SKIP_INSTALLER=1)"
else
    # Windows' VERSIONINFO is four numbers; the tag is one, two or three.
    quad="$(printf '%s' "$VERSION" | tr -d 'v' | awk -F'[.-]' '{printf "%d.%d.%d.%d", $1, $2, $3, $4}')"
    echo "==> building the installer  ($SETUP_NAME, VIProductVersion $quad)"
    rm -f "$BUILD/$SETUP_NAME"
    run_makensis "$quad"
    [ -f "$BUILD/$SETUP_NAME" ] || { echo "error: makensis produced no $SETUP_NAME" >&2; exit 1; }
fi

echo
echo "==> done"
echo "    package:   $PKG"
echo "    zip:       $BUILD/$PKG_NAME.zip"
[ -f "$BUILD/$SETUP_NAME" ] && echo "    installer: $BUILD/$SETUP_NAME"
exit 0
