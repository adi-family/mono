#!/usr/bin/env bash
# Build and serve the PUBLIC marketplace — the pages a person who has never installed adi (and
# Google) can read — against this repo's dev fixtures, on http://127.0.0.1:9085.
#
# The fixtures are the five bundles `scripts/dev-marketplace-fixtures.sh` writes into the isolated
# dev store; run that first if .adi-dev/fixtures/marketplace.json is not there yet. Nothing here
# touches the live store, and nothing is published: the site is held in memory and served on
# loopback until Ctrl-C.
#
# To build the real thing instead, point it at the manifest you publish and say where it will
# live (the generator refuses to invent either):
#
#   cargo run -p adi-market-site -- build \
#     --source adi=~/adi-family-marketplace/apps/marketplace.json \
#     --source-url adi=https://raw.githubusercontent.com/adi-family/marketplace/main/apps/marketplace.json \
#     --base-url https://marketplace.withadi.dev --out dist
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

manifest="${ADI_MARKET_MANIFEST:-$repo_root/.adi-dev/fixtures/marketplace.json}"
port="${ADI_MARKET_PORT:-9085}"
name="${ADI_FIXTURE_SOURCE:-bundles}"

[ -f "$manifest" ] || {
  echo "error: no manifest at $manifest — run scripts/dev-marketplace-fixtures.sh first," >&2
  echo "       or set ADI_MARKET_MANIFEST to one you have." >&2
  exit 1
}

# Built rather than `cargo run`, so the line that prints the URL is the first thing on screen
# instead of arriving after a minute of compiler output.
cargo build -p adi-market-site
exec ./target/debug/adi-market-site serve \
  --source "$name=$manifest" \
  --source-url "$name=file://$manifest" \
  --port "$port"
