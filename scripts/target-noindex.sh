#!/usr/bin/env bash
#
# Keep cargo's build output out of Spotlight: move `target/` to `target.noindex/` and leave
# `target` as a symlink to it. macOS only; anywhere else this does nothing.
#
# Why: every build writes thousands of fresh files under `target/`, and Spotlight indexes each
# one. Measured 2026-10-04 on a machine building this repo: 154,386 of the clone's 201,903 indexed
# items were under `target/`, `mds_stores` sat at 100-160% CPU, and the disk was saturated enough
# that the control panel took 10 s to draw a page. Spotlight skips a folder named `*.noindex` and
# reads no further than the real path, so a visible symlink costs it nothing, and every
# `target/release/...` path this repo documents and runs from keeps resolving.
#
# Ruled out on macOS 26, by indexing a probe file: a `.metadata_never_index` file inside the
# folder (honoured on volumes only) and `chflags hidden` (Finder only). Both were indexed anyway.
#
# Run for you by the post-checkout hook, by scripts/install-hooks.sh and by scripts/build-app.sh.
# Idempotent: once `target` is the symlink, this is one `stat`.
#
#   scripts/target-noindex.sh
#
# One caveat: `cargo clean` removes only the symlink and leaves the build output behind in
# `target.noindex/`, and the next build makes a fresh, indexed `target/`. The next run of this
# script finds that pair, deletes the stale `target.noindex/` (it is what the clean was asked to
# delete) and moves the new one into its place. For a full clean right now: `rm -rf target.noindex`.
set -euo pipefail

[ "$(uname -s)" = "Darwin" ] || exit 0

# The checkout this script belongs to — a worktree carries its own copy, and its own `target/` —
# whatever directory it was called from.
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$(git -C "$here" rev-parse --show-toplevel 2>/dev/null || dirname "$here")"

real="target.noindex"

# Already done, and the usual case.
[ -L target ] && exit 0

# A build in flight holds a lock under `target/`, and moving the directory out from under it could
# let it recreate a real `target/` between the move and the link. Next run, then.
if [ -d target ] && command -v lsof >/dev/null 2>&1; then
    locks=()
    for lock in target/*/.cargo-lock target/*/*/.cargo-lock; do
        [ -e "$lock" ] && locks+=("$lock")
    done
    # One `lsof` for all of them: each call walks every process on the machine.
    if [ "${#locks[@]}" -gt 0 ] && lsof -t "${locks[@]}" >/dev/null 2>&1; then
        echo "target-noindex: a build is running under target/; leaving it where it is for now" >&2
        exit 0
    fi
fi

if [ -d target ] && [ -e "$real" ]; then
    # Both exist: `cargo clean` unlinked `target` and a build has since made a real one. Delete the
    # stale copy only if it is unmistakably cargo's own output, never a folder someone made.
    if [ -e "$real/.rustc_info.json" ] || [ -e "$real/CACHEDIR.TAG" ]; then
        echo "target-noindex: removing build output left behind by cargo clean ($real/)"
        rm -rf "$real"
    else
        echo "target-noindex: both target/ and $real/ exist and $real/ is not cargo output;" >&2
        echo "                not touching either. Move one aside and run this again." >&2
        exit 0
    fi
fi

if [ -d target ]; then
    # A rename within one filesystem: instant whatever the size, and a binary already running out
    # of target/release keeps running.
    mv target "$real"
else
    mkdir -p "$real"
fi
ln -s "$real" target
echo "target-noindex: target -> $real (kept out of Spotlight)"
