#!/usr/bin/env bash
# The custom elements in design/elements, turned into what a browser loads — and type-checked.
#
#   scripts/elements.sh build [OUT]   # OUT defaults to target/elements
#   scripts/elements.sh check         # tsc over design/elements; the build never checks
#
# The sources are TypeScript where they have been converted and JavaScript where they have not.
# `build` writes one file out per file in: each `x.ts` becomes `x.js` with its types stripped and
# any `.css` it imports inlined as text; each `x.js` is copied as it is. Nothing is merged, so every
# file keeps its own URL (`/elements/chat.js`) and every relative import — written `./base.js` in
# TypeScript too — resolves to the file it always did. That is why the agent board, and any page
# that loads `/elements/…` from a panel, needs no change.
#
# Trunk runs `build` before every build and every serve rebuild (crates/adi-webapp/Trunk.toml),
# and index.html copies OUT into dist as `/elements/`. OUT is under target/, which Trunk does not
# watch: written anywhere under design/, every build would set off the next.
#
# esbuild strips types; it never checks them. `check` does, with the TypeScript pinned below, and
# CI runs it before the webapp is built — so a type error cannot ship, while a rebuild still costs
# no more than a file save.
set -euo pipefail
shopt -s nullglob

ESBUILD_VERSION="0.28.0" # what CI installs; locally, whichever esbuild is on PATH
TYPESCRIPT_VERSION="7.0.2"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/design/elements"

build() {
  local out="${1:-$ROOT/target/elements}"
  # It is emptied first, so it had better be ours: dist copies it by its name.
  if [[ "$(basename "$out")" != "elements" ]]; then
    echo "elements: refusing to empty $out — the output directory must be named 'elements'" >&2
    exit 1
  fi
  if ! command -v esbuild >/dev/null; then
    echo "elements: esbuild is not on PATH — brew install esbuild, or npm install -g esbuild@$ESBUILD_VERSION" >&2
    exit 1
  fi
  rm -rf "$out"
  mkdir -p "$out"

  local ts=() f name
  for f in "$SRC"/*.ts; do
    [[ "$f" == *.d.ts ]] || ts+=("$f")
  done
  if ((${#ts[@]})); then
    # `--bundle` only so an imported `.css` is followed and inlined. Every relative `.js` import
    # is external, so no file is ever folded into another.
    esbuild "${ts[@]}" --bundle --format=esm --target=es2022 --outdir="$out" \
      --external:'./*.js' --loader:.css=text --sourcemap --log-level=warning
  fi
  for f in "$SRC"/*.js; do
    name="$(basename "$f")"
    if [[ -e "$out/$name" ]]; then
      echo "elements: both ${name%.js}.ts and $name exist — one of them is stale" >&2
      exit 1
    fi
    cp "$f" "$out/"
  done
  echo "elements: ${#ts[@]} compiled from TypeScript, $(find "$SRC" -maxdepth 1 -name '*.js' | wc -l | tr -d ' ') copied → $out"
}

check() {
  cd "$SRC"
  npx --yes -p "typescript@$TYPESCRIPT_VERSION" tsc -p .
}

case "${1:-}" in
  build) build "${2:-}" ;;
  check) check ;;
  *)
    echo "usage: scripts/elements.sh build [OUT] | check" >&2
    exit 2
    ;;
esac
