#!/usr/bin/env bash
# Build browser modules or check types in design/elements.
set -euo pipefail
shopt -s nullglob

ESBUILD_VERSION="0.28.0" # what CI installs; locally, whichever esbuild is on PATH
TYPESCRIPT_VERSION="7.0.2"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$ROOT/design/elements"

typescript() {
  npx --yes --prefer-offline -p "typescript@$TYPESCRIPT_VERSION" tsc "$@"
}

build() (
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

  build_stage="$(mktemp -d "${TMPDIR:-/tmp}/adi-elements.XXXXXX")"
  build_stage="$(cd "$build_stage" && pwd -P)"
  trap 'rm -rf "$build_stage"' EXIT

  # TypeScript rewrites .ts imports; esbuild inlines the shared stylesheet.
  typescript -p "$SRC" --noEmit false --noCheck --outDir "$build_stage" --sourceMap --inlineSources
  cp "$SRC"/*.css "$build_stage/"
  local modules=("$build_stage"/*.js)
  esbuild "${modules[@]}" --bundle --format=esm --target=es2022 --outdir="$out" \
    --external:'./*.js' --loader:.css=text --sourcemap --log-level=warning

  echo "elements: ${#modules[@]} compiled from TypeScript → $out"
)

check() {
  typescript -p "$SRC"
}

case "${1:-}" in
  build) build "${2:-}" ;;
  check) check ;;
  *)
    echo "usage: scripts/elements.sh build [OUT] | check" >&2
    exit 2
    ;;
esac
