#!/usr/bin/env bash
#
# Lift one version's section out of CHANGELOG.md.
#
# Usage:
#   scripts/changelog.sh 0.3.0        # the body of "## 0.3.0 — <date>", heading excluded
#   scripts/changelog.sh v0.3.0       # same — a leading v is accepted, since tags carry one
#   scripts/changelog.sh Unreleased   # what has landed but not shipped
#   scripts/changelog.sh --list       # the versions the file has sections for
#
# What comes out is the release notes, used three times over: the `notes` field of the
# published manifest.json, the body of the GitHub release, and the "What's new" the control
# panel shows before it installs anything (docs/adi-update.md §5).
#
# **A missing section is an error, not an empty string.** The release workflow runs this
# before it builds anything, so forgetting to write the entry — or forgetting to rename
# `## Unreleased` at tag time, which looks exactly the same from here — stops the release in
# ten seconds instead of publishing a version whose notes read "ADI v0.3.0" to every machine
# that takes it.
set -euo pipefail

FILE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/CHANGELOG.md"

die() { echo "error: $*" >&2; exit 1; }

[ -f "$FILE" ] || die "no CHANGELOG.md at $FILE"

# Every version heading, in file order. A heading is `## <token>[ …]`, and the token is the
# version — the em-dashed date after it is for the reader, not for this script.
headings() { sed -n 's/^## \([^ ]*\).*$/\1/p' "$FILE"; }

case "${1:-}" in
    --list) headings; exit 0 ;;
    "")     die "usage: changelog.sh <version>|--list" ;;
    -*)     die "unknown flag $1" ;;
esac

# `v0.3.0` and `0.3.0` name the same section: the tag carries the v, `scripts/version.sh` does
# not, and both call this.
WANT="${1#v}"

# Print the lines between this version's heading and the next `## ` heading. `### Added` and
# friends are not headings by this rule — they are the section's own structure.
body="$(
    awk -v want="$WANT" '
        /^## / {
            if (inside) exit
            hdr = substr($0, 4)
            sub(/[[:space:]].*$/, "", hdr)
            if (hdr == want) { inside = 1 }
            next
        }
        inside { print }
    ' "$FILE" | sed '/./,$!d'
)"

if [ -z "$body" ]; then
    echo "error: CHANGELOG.md has no section for $WANT." >&2
    echo "       Add '## $WANT — $(date -u +%Y-%m-%d)' — renaming '## Unreleased' if that is" >&2
    echo "       what this release is. Sections present: $(headings | tr '\n' ' ')" >&2
    exit 1
fi

printf '%s\n' "$body"
