#!/usr/bin/env bash
# The ADI Store — the public pages a person who has never installed adi (and Google) can read.
#
#   scripts/store.sh                 look at it, on http://127.0.0.1:9085
#   scripts/store.sh --into-landing  write it into the landing checkout, as withadi.dev/store
#
# The store is a directory of plain HTML with relative links throughout, which is what lets the
# same output serve at a host root and inside a bigger site. On withadi.dev it is the second:
# Astro copies `public/` into `dist/` verbatim, so `public/store/` is served at `/store`.
#
# **What it is built from.** By default the five fixture bundles
# `scripts/dev-marketplace-fixtures.sh` writes into the isolated dev store — enough to look at,
# and not something to publish. Point ADI_STORE_MANIFEST at the real manifest to build the real
# thing, and pass its published URL as ADI_STORE_MANIFEST_URL so the pages can print the
# `marketplace add` line.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

manifest="${ADI_STORE_MANIFEST:-$repo_root/.adi-dev/fixtures/marketplace.json}"
manifest_url="${ADI_STORE_MANIFEST_URL:-file://$manifest}"
# The marketplace's local name. It is the first half of every install address
# (`marketplace install store/crm-suite`), and the operator's own to choose; `store` is what the
# published one is meant to be added as.
name="${ADI_STORE_NAME:-store}"
port="${ADI_STORE_PORT:-9085}"
# Where the landing is checked out. Its own repo (adi-family/withadi.dev), not this one.
landing="${ADI_LANDING:-$HOME/.adi/mono/projects/adi-landing/workspaces/main}"

[ -f "$manifest" ] || {
  echo "error: no manifest at $manifest — run scripts/dev-marketplace-fixtures.sh first," >&2
  echo "       or set ADI_STORE_MANIFEST to the one you publish." >&2
  exit 1
}

# Built rather than `cargo run`, so the line that prints the URL is the first thing on screen
# instead of arriving after a minute of compiler output.
cargo build -p adi-market-site

if [ "${1:-}" = "--into-landing" ]; then
  [ -d "$landing/public" ] || {
    echo "error: no landing checkout at $landing (set ADI_LANDING)" >&2
    exit 1
  }
  exec ./target/debug/adi-market-site build \
    --source "$name=$manifest" \
    --source-url "$name=$manifest_url" \
    --base-url "https://withadi.dev/store" \
    --out "$landing/public/store"
fi

exec ./target/debug/adi-market-site serve \
  --source "$name=$manifest" \
  --source-url "$name=$manifest_url" \
  --port "$port"
