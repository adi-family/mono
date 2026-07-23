#!/usr/bin/env bash
#
# Build the Windows ADI package: cross-compile the four platform binaries for
# x86_64-pc-windows-gnu, assemble them with a launcher + README, and zip it.
#
# There is no native Windows GUI (the macOS app is Swift/AppKit); on Windows the control
# panel is the web UI that `adi-app` already serves, opened in a browser by the launcher.
#
# Output:  apps/windows/build/ADI-windows-x64/       (the unpacked package)
#          apps/windows/build/ADI-windows-x64.zip     (the shippable archive)
#
# Requirements (on the build host):
#   rustup target add x86_64-pc-windows-gnu
#   the mingw-w64 cross toolchain (`x86_64-w64-mingw32-gcc` on PATH) — `brew install mingw-w64`
#   zip
#
# Run from macOS/Linux; it cross-compiles. Set SKIP_BUILD=1 to only re-assemble from an
# existing target/ (useful when iterating on the launcher/README).
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

TARGET="x86_64-pc-windows-gnu"
PKG_NAME="ADI-windows-x64"
BUILD="$SCRIPT_DIR/build"
PKG="$BUILD/$PKG_NAME"
BINS=(adi-mono adi-dns adi-hive adi-app)

# The workspace version is the single source of truth (root Cargo.toml), same as the macOS build.
VERSION="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n1)"
[ -n "$VERSION" ] || { echo "error: workspace version not found in $ROOT/Cargo.toml" >&2; exit 1; }

if [ "${SKIP_BUILD:-}" != "1" ]; then
    command -v x86_64-w64-mingw32-gcc >/dev/null || {
        echo "error: x86_64-w64-mingw32-gcc not found — install the mingw-w64 cross toolchain" >&2
        echo "       (macOS: brew install mingw-w64)" >&2
        exit 1
    }
    echo "==> cross-compiling for $TARGET (release): ${BINS[*]}"
    ( cd "$ROOT" && cargo build --release --target "$TARGET" \
        -p adi-cli -p adi-dns -p adi-hive -p adi-app )
fi

# Verify every .exe exists before packaging.
OUT="$ROOT/target/$TARGET/release"
for b in "${BINS[@]}"; do
    [ -f "$OUT/$b.exe" ] || { echo "error: $OUT/$b.exe missing (build failed?)" >&2; exit 1; }
done

echo "==> assembling $PKG  (version $VERSION)"
rm -rf "$PKG"
mkdir -p "$PKG"
for b in "${BINS[@]}"; do
    cp "$OUT/$b.exe" "$PKG/$b.exe"
done
echo "$VERSION" > "$PKG/VERSION"

# ── Launcher: bring services up, then open the control panel in the browser ──
# `adi-mono up` enables/starts every service (registering per-user Task Scheduler tasks). The
# control panel port is allocated dynamically, so read it back from `status --json` (the `app`
# service's `detail` carries `127.0.0.1:<port>`) rather than hard-coding a URL.
cat > "$PKG/start-adi.ps1" <<'PS1'
$ErrorActionPreference = 'SilentlyContinue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$adi  = Join-Path $here 'adi-mono.exe'

Write-Host 'Starting ADI services...'
& $adi up | Out-Null

$url = 'http://app.adi/'
try {
    $status = & $adi status --json | ConvertFrom-Json
    $app = $status.services | Where-Object { $_.id -eq 'app' } | Select-Object -First 1
    if ($app -and $app.detail -match '127\.0\.0\.1:(\d+)') {
        $url = "http://127.0.0.1:$($Matches[1])/"
    }
} catch { }

Write-Host "Opening control panel: $url"
Start-Process $url
& $adi status
PS1

cat > "$PKG/Start ADI.cmd" <<'CMD'
@echo off
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0start-adi.ps1"
CMD

cat > "$PKG/Stop ADI.cmd" <<'CMD'
@echo off
"%~dp0adi-mono.exe" disable
echo ADI services stopped.
pause
CMD

# `adi` convenience shim so the CLI can be invoked as `adi ...` (the platform's target name).
cat > "$PKG/adi.cmd" <<'CMD'
@echo off
"%~dp0adi-mono.exe" %*
CMD

# Optional: add this folder to the *user* PATH (no admin needed). The `harness:adi` agent backend
# re-invokes `adi-mono` by name, so having it on PATH makes those agents work from anywhere.
cat > "$PKG/Add ADI to PATH.cmd" <<'CMD'
@echo off
setlocal
set "DIR=%~dp0"
if "%DIR:~-1%"=="\" set "DIR=%DIR:~0,-1%"
powershell -NoProfile -ExecutionPolicy Bypass -Command ^
  "$d='%DIR%'; $p=[Environment]::GetEnvironmentVariable('Path','User'); if (($p -split ';') -notcontains $d) { [Environment]::SetEnvironmentVariable('Path', ($p.TrimEnd(';') + ';' + $d), 'User'); Write-Host 'Added to PATH (open a new terminal to pick it up).' } else { Write-Host 'Already on PATH.' }"
pause
CMD

cp "$SCRIPT_DIR/README.md" "$PKG/README.txt"

echo "==> zipping"
rm -f "$BUILD/$PKG_NAME.zip"
( cd "$BUILD" && zip -qr "$PKG_NAME.zip" "$PKG_NAME" )

echo
echo "==> done"
echo "    package: $PKG"
echo "    zip:     $BUILD/$PKG_NAME.zip"
