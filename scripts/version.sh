#!/usr/bin/env sh
#
# Print the version this build stamps into its artifacts. **The git tag is the source of
# truth** — `git tag v0.2.0 && git push --tags` is what cuts a release, and every packaging
# script (macOS, Linux, Windows) and the compiled-in `BUILT_VERSION` read it from here so
# the three can never disagree.
#
# Resolution order:
#
#   1. $ADI_VERSION            — set by CI from the pushed tag (`v0.2.0` → `0.2.0`).
#   2. the tag on HEAD         — a local release build from a tagged commit.
#   3. the nearest tag         — a dev build between releases. It reports the *released*
#                                version deliberately: `Version::is_newer` is strict, so a
#                                dev build is never "older" than the release it came after
#                                and no auto-update overwrites a work-in-progress bundle,
#                                while the next real release still lands normally.
#   4. the workspace version   — no tags at all, or no git (a source tarball).
#
# The result must parse as `major[.minor[.patch]]`: `adi-update`'s comparison fails closed on
# anything else, so a build stamped `0.2.0-dirty` would silently never update again. Better to
# fail here, loudly, than to ship a machine that has quietly stopped receiving updates.
#
# POSIX sh: apps/linux/install.sh's constraints apply to anything a node might run.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"

strip_v() { printf '%s' "${1#v}"; }

resolve() {
    if [ -n "${ADI_VERSION:-}" ]; then
        strip_v "$ADI_VERSION"
        return
    fi
    if git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
        if tag="$(git -C "$ROOT" describe --tags --exact-match HEAD 2>/dev/null)"; then
            strip_v "$tag"
            return
        fi
        if tag="$(git -C "$ROOT" describe --tags --abbrev=0 2>/dev/null)"; then
            strip_v "$tag"
            return
        fi
    fi
    sed -n 's/^version = "\(.*\)"$/\1/p' "$ROOT/Cargo.toml" | head -n1
}

VERSION="$(resolve)"

[ -n "$VERSION" ] || {
    echo "error: could not resolve a version (no ADI_VERSION, no git tag, no workspace version)" >&2
    exit 1
}

# Reject anything adi-update's Version::parse would refuse, for the reason in the header.
case "$VERSION" in
    *[!0-9.]*|*..*|.*|*.)
        echo "error: resolved version '$VERSION' is not major[.minor[.patch]]" >&2
        echo "       tag releases as v0.2.0 — adi-update ignores anything else and would" >&2
        echo "       leave this build permanently un-updatable" >&2
        exit 1
        ;;
esac

printf '%s\n' "$VERSION"
