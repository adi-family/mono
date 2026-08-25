#!/usr/bin/env bash
# Rasterise the app tile into the PNGs a browser and a manifest still ask for.
#
# The drawing lives in mark.svg and mark-maskable.svg, which are generated from `Mark` in
# crates/adi-ui/src/mark.rs — edit those, never the PNGs. Nothing runs this automatically: the
# icons change about once a year and a build step that shells out to a rasteriser would make the
# whole webapp unbuildable on a machine without one.
#
# Two families, because the slots are not the same shape. The `any` icons keep the squircle and
# the margin around it, which is what a browser tab and a desktop shortcut draw as-is. The
# `maskable` ones bleed to the edges and hold the mark inside the centre 80%, because a launcher
# crops them to its own shape — a squircle inside a circle is a smaller, wonkier squircle.
#
# apple-touch-icon is maskable rather than `any` for the same reason and one more: iOS rounds the
# corners itself and composites transparency onto black, so a tile with clear corners arrives
# with black ones.
#
#   ./regen-icons.sh
set -euo pipefail

cd "$(dirname "$0")"

command -v rsvg-convert >/dev/null || {
  echo "regen-icons: rsvg-convert not found (brew install librsvg)" >&2
  exit 1
}

render() {
  local src=$1 size=$2 out=$3
  rsvg-convert -w "$size" -h "$size" "$src" -o "$out"
  echo "  $out  ${size}×${size}"
}

echo "from mark.svg:"
render mark.svg 32 favicon.png
render mark.svg 192 icon-192.png
render mark.svg 512 icon-512.png

echo "from mark-maskable.svg:"
render mark-maskable.svg 180 apple-touch-icon.png
render mark-maskable.svg 192 icon-maskable-192.png
render mark-maskable.svg 512 icon-maskable-512.png
