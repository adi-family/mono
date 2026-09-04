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

# Same version rule as the three packaging scripts (scripts/version.sh): without it a dev
# build reports the workspace floor, so a control panel run from target/ claims 0.1.0 while
# the bundle beside it is on the released version — which reads as a failed auto-update and
# is not one.
ADI_VERSION="$("$repo_root/scripts/version.sh")"
export ADI_VERSION
echo "==> version: $ADI_VERSION"

if ! command -v trunk >/dev/null 2>&1; then
  echo "error: 'trunk' is not installed. Install it with:  brew install trunk" >&2
  echo "       (or: cargo install trunk)" >&2
  exit 1
fi

# Every local asset dist/index.html asks for has to be *in* dist. A missing one is not a build
# error anywhere — trunk succeeds, cargo succeeds, the binary runs, the health endpoint answers —
# and the panel serves the SPA fallback in place of the stylesheet, so the app comes up with its
# markup intact and no layout at all.
#
# It happens for one reason: `trunk serve` (the dev-ui hive service) writes this same dist, and its
# partial dev output can land between the two builds below, leaving an index.html from one build
# beside the assets of another. The staging below keeps it out of the long half; this check covers
# the short one, because whatever is in dist when `include_dir!` reads it is what shipped.
check_dist() {
  local dist="$repo_root/crates/adi-webapp/dist" missing=0 ref
  for ref in $(grep -o '\(href\|src\)="/[^"]*"' "$dist/index.html" | sed 's/.*"\/\(.*\)"/\1/'); do
    [ -e "$dist/$ref" ] || { echo "   missing: $ref" >&2; missing=1; }
  done
  [ "$missing" -eq 0 ] || {
    echo "error: dist/index.html references assets that are not in dist ($1)" >&2
    echo "       the panel would serve its SPA fallback in their place — an app with no styles" >&2
    echo "       the dev server rewrote dist between the staging and cargo; just build again." >&2
    echo "       (killing dev-ui does not help: the supervisor restarts it within seconds and a" >&2
    echo "        fresh trunk serve rebuilds straight into dist, making a collision *more* likely.)" >&2
    exit 1
  }
}

# Trunk builds into a dist of this build's own, under target/, and it is swapped into place only
# once it is complete. The shared crates/adi-webapp/dist is *also* what the dev-ui service's
# `trunk serve` writes — Trunk.toml gives serve and build one dist — and its rebuild deletes the
# stage directory out from under a build using it. That fails minutes in, nowhere near the cause,
# as "error writing JS loader file to stage dir: No such file or directory". Nothing watches
# target/, so the long half of the build cannot collide at all.
private_dist="$repo_root/target/webapp-dist"
echo "==> trunk build ${trunk_flags[*]}  (crates/adi-webapp -> target/webapp-dist/)"
rm -rf "$private_dist"
( cd crates/adi-webapp && trunk build "${trunk_flags[@]}" --dist "$private_dist" )

echo "==> staging it as crates/adi-webapp/dist/"
rm -rf "$repo_root/crates/adi-webapp/dist"
cp -R "$private_dist" "$repo_root/crates/adi-webapp/dist"
check_dist "after trunk build"

echo "==> cargo build ${cargo_flags[*]} -p adi-app  (embeds dist/)"
cargo build "${cargo_flags[@]}" -p adi-app
check_dist "after cargo build — this is what got embedded"

echo "==> built: $repo_root/target/$profile/adi-app"
