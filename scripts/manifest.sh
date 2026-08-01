#!/usr/bin/env bash
#
# Write the release `manifest.json` — the small file every installed copy of adi polls to
# learn that a new version exists, and the only thing that has to be published for a machine
# to update itself.
#
# Usage:
#   scripts/manifest.sh --version 0.2.0 --base-url https://github.com/o/r/releases/download/v0.2.0 \
#       --artifact macos=build/ADI.dmg \
#       --artifact linux-x86_64=build/adi-linux-x64.tar.gz \
#       --artifact windows-x86_64=build/ADI-windows-x64.zip \
#       [--notes "what changed"] [--url-for macos=https://elsewhere/ADI.dmg] > manifest.json
#
# Each --artifact is `<platform>=<local file>`: the file is hashed and measured here, and its
# published URL defaults to `<base-url>/<basename>`. `--url-for` overrides that for a platform
# whose asset is hosted somewhere else.
#
# Platform keys must match what the client asks for (`adi_update::host_platform`): `macos` for
# the universal bundle, `<os>-<arch>` everywhere else. A key nothing looks up publishes an
# artifact no machine will ever download.
#
# The `macos` artifact is *also* emitted as the legacy top-level `dmg` field, because clients
# released before per-platform artifacts existed require it and would otherwise stop updating.
set -euo pipefail

VERSION=""
BASE_URL=""
NOTES=""
declare -a PLATFORMS=() FILES=()
declare -A URL_OVERRIDE=()

die() { echo "error: $*" >&2; exit 1; }

while [ $# -gt 0 ]; do
    case "$1" in
        --version)  VERSION="${2:?--version needs a value}"; shift 2 ;;
        --base-url) BASE_URL="${2:?--base-url needs a value}"; shift 2 ;;
        --notes)    NOTES="${2:?--notes needs a value}"; shift 2 ;;
        --artifact)
            spec="${2:?--artifact needs <platform>=<file>}"; shift 2
            [ "${spec%%=*}" != "$spec" ] || die "--artifact must be <platform>=<file>, got '$spec'"
            PLATFORMS+=("${spec%%=*}")
            FILES+=("${spec#*=}")
            ;;
        --url-for)
            spec="${2:?--url-for needs <platform>=<url>}"; shift 2
            [ "${spec%%=*}" != "$spec" ] || die "--url-for must be <platform>=<url>, got '$spec'"
            URL_OVERRIDE["${spec%%=*}"]="${spec#*=}"
            ;;
        *) die "unknown flag $1" ;;
    esac
done

[ -n "$VERSION" ]       || die "--version is required"
[ ${#PLATFORMS[@]} -gt 0 ] || die "at least one --artifact is required"

# sha256 and size, under whichever names this host has for them. The release is cut on macOS
# (BSD tools) or a Linux CI runner (GNU), and the manifest must come out identical either way.
sha256_of() {
    if command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        sha256sum "$1" | awk '{print $1}'
    fi
}
size_of() { stat -f%z "$1" 2>/dev/null || stat -c%s "$1"; }

json_escape() { printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g' -e 's/\t/\\t/g' | awk '{printf "%s%s", sep, $0; sep="\\n"} END {print ""}'; }

PUB_DATE="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
NOTES_JSON="$(json_escape "${NOTES:-ADI v$VERSION}")"

# Collect each artifact's facts first, so a missing file fails before anything is printed.
declare -a ENTRIES=()
MACOS_ENTRY=""
for i in "${!PLATFORMS[@]}"; do
    platform="${PLATFORMS[$i]}"
    file="${FILES[$i]}"
    [ -f "$file" ] || die "artifact for $platform not found: $file"
    url="${URL_OVERRIDE[$platform]:-}"
    if [ -z "$url" ]; then
        [ -n "$BASE_URL" ] || die "no --base-url and no --url-for $platform, so its URL is unknown"
        url="${BASE_URL%/}/$(basename "$file")"
    fi
    body="{ \"url\": \"$url\", \"sha256\": \"$(sha256_of "$file")\", \"size\": $(size_of "$file") }"
    ENTRIES+=("    \"$platform\": $body")
    [ "$platform" = "macos" ] && MACOS_ENTRY="$body"
done

{
    echo "{"
    echo "  \"version\": \"$VERSION\","
    echo "  \"pub_date\": \"$PUB_DATE\","
    echo "  \"notes\": \"$NOTES_JSON\","
    # Legacy field, kept for clients that predate `artifacts` — see the header.
    [ -n "$MACOS_ENTRY" ] && echo "  \"dmg\": $MACOS_ENTRY,"
    echo "  \"artifacts\": {"
    for i in "${!ENTRIES[@]}"; do
        if [ "$i" -eq $(( ${#ENTRIES[@]} - 1 )) ]; then echo "${ENTRIES[$i]}"; else echo "${ENTRIES[$i]},"; fi
    done
    echo "  }"
    echo "}"
}
