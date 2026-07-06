#!/usr/bin/env bash
# Build the whole adi app in one shot: compile the Leptos UI (adi-webapp) to wasm with
# Trunk, then build adi-app, which embeds Trunk's dist/ at compile time.
#
# Usage: scripts/build-app.sh [--debug]   (default: release)
set -euo pipefail

profile="release"
trunk_flags=(--release)
cargo_flags=(--release)
if [[ "${1:-}" == "--debug" ]]; then
  profile="debug"
  trunk_flags=()
  cargo_flags=()
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! command -v trunk >/dev/null 2>&1; then
  echo "error: 'trunk' is not installed. Install it with:  brew install trunk" >&2
  echo "       (or: cargo install trunk)" >&2
  exit 1
fi

echo "==> trunk build ${trunk_flags[*]}  (crates/adi-webapp -> dist/)"
( cd crates/adi-webapp && trunk build "${trunk_flags[@]}" )

echo "==> cargo build ${cargo_flags[*]} -p adi-app  (embeds dist/)"
cargo build "${cargo_flags[@]}" -p adi-app

echo "==> built: $repo_root/target/$profile/adi-app"
