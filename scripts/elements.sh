#!/usr/bin/env bash
# Build browser modules or check types in design/elements.
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

  local ts=() f
  for f in "$SRC"/*.ts; do
    [[ "$f" == *.d.ts ]] || ts+=("$f")
  done
  if ((${#ts[@]})); then
    # Inline the shared stylesheet; keep JavaScript imports as separate modules.
    esbuild "${ts[@]}" --bundle --format=esm --target=es2022 --outdir="$out" \
      --external:'./*.js' --loader:.css=text --sourcemap --log-level=warning
  fi
  echo "elements: ${#ts[@]} compiled from TypeScript → $out"
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
