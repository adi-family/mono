#!/usr/bin/env bash
# Build the browser mesh client into a deployable `dist/`.
#
# The artefact is static files and nothing else — there is no server half. Everything the client
# shows it fetched itself over QUIC from a machine that is listening on nothing, so "deploying" it
# is copying `crates/adi-mesh-client/dist/` onto any static host that serves it over **HTTPS**
# (see that crate's DEPLOY.md; a service worker and `crypto.subtle` need a secure context).
#
# Usage: scripts/build-mesh-client.sh [--debug]   (default: release)
#
# Why this exists rather than a bare `trunk build`: cargo reads `.cargo/config.toml` from the
# **current directory** upward, and the one that points cc-rs at Homebrew's LLVM lives in the crate
# directory. Run from the repo root, `ring` fails to compile its C for wasm32 with "unable to
# create target". So the one thing this script must do is `cd` first.
set -euo pipefail

trunk_flags=(--release)
[[ "${1:-}" == "--debug" ]] && trunk_flags=()

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
crate="$repo_root/crates/adi-mesh-client"

if ! command -v trunk >/dev/null 2>&1; then
  echo "error: 'trunk' is not installed. Install it with:  brew install trunk" >&2
  echo "       (or: cargo install trunk)" >&2
  exit 1
fi
if ! rustup target list --installed | grep -qx wasm32-unknown-unknown; then
  echo "==> adding the wasm32-unknown-unknown target"
  rustup target add wasm32-unknown-unknown
fi
if [[ "$(uname -s)" == "Darwin" && ! -x /opt/homebrew/opt/llvm/bin/clang ]]; then
  # Apple's clang has no wasm backend, and `ring` compiles C for every target it supports.
  echo "error: Homebrew LLVM is missing, and 'ring' cannot compile for wasm without it." >&2
  echo "       brew install llvm" >&2
  echo "       (see crates/adi-mesh-client/.cargo/config.toml for what it is used for)" >&2
  exit 1
fi

cd "$crate"
echo "==> building $crate"
trunk build "${trunk_flags[@]}"

wasm="$(find dist -name '*_bg.wasm' | head -1)"
if [[ -n "$wasm" ]]; then
  raw=$(stat -f%z "$wasm" 2>/dev/null || stat -c%s "$wasm")
  # Reported because this is served to phones over mobile data, and the number is the reason to
  # care what goes into the bundle. Brotli is what a static host actually sends.
  br=$(brotli -q 11 -c "$wasm" 2>/dev/null | wc -c | tr -d ' ' || echo "?")
  echo "==> wasm: $((raw / 1024)) KB raw, $((br / 1024)) KB brotli"
fi
echo "==> dist: $crate/dist"
