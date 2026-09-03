#!/usr/bin/env bash
# The Lucide icons the UI uses, fetched into crates/adi-ui/icons/ at one pinned release.
#
#   scripts/lucide.sh              # fetch every name in crates/adi-ui/icons/ICONS that is missing
#   scripts/lucide.sh --all        # refetch everything (after bumping VERSION)
#   scripts/lucide.sh add <name>…  # append names to ICONS, sorted, and fetch them
#
# The SVGs land verbatim, licence header included; `crates/adi-ui/build.rs` reads the directory
# and generates the `adi_ui::Lucide` enum from it, so adding an icon is: add the name here, run
# this, build. Names are checked against lucide.dev — a typo is a 404, never a silent blank.
set -euo pipefail

VERSION="1.39.0"
CDN="https://cdn.jsdelivr.net/npm/lucide-static@${VERSION}/icons"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
DIR="$ROOT/crates/adi-ui/icons"
LIST="$DIR/ICONS"

names() { grep -v '^\s*#' "$LIST" | sed '/^\s*$/d'; }

fetch() {
  local name="$1" out="$DIR/$1.svg" code
  code=$(curl -sS -o "$out.tmp" -w '%{http_code}' "$CDN/$name.svg")
  if [ "$code" != "200" ]; then
    rm -f "$out.tmp"
    echo "lucide: no icon named '$name' in lucide-static ${VERSION} (HTTP $code)" >&2
    return 1
  fi
  mv "$out.tmp" "$out"
  echo "  $name"
}

case "${1:-}" in
  add)
    shift
    for n in "$@"; do
      grep -qx "$n" "$LIST" || echo "$n" >> "$LIST"
    done
    { grep '^\s*#' "$LIST"; names | sort -u; } > "$LIST.tmp" && mv "$LIST.tmp" "$LIST"
    for n in "$@"; do fetch "$n"; done
    ;;
  --all)
    for n in $(names); do fetch "$n"; done
    ;;
  *)
    for n in $(names); do [ -f "$DIR/$n.svg" ] || fetch "$n"; done
    ;;
esac
