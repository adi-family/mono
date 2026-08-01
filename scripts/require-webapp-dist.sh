#!/usr/bin/env sh
#
# Refuse to package a build whose control panel has no UI.
#
# `crates/adi-app/build.rs` *creates* `crates/adi-webapp/dist` when it is missing, because
# `include_dir!` needs the directory to exist on a fresh checkout. That kindness has a sharp
# edge at release time: a packaging run that forgets `trunk build` compiles cleanly, embeds an
# empty directory, and ships a control panel that serves a placeholder instead of the app.
# Nothing downstream notices — the binary runs, the service starts, the health endpoint answers.
#
# So every packaging script calls this first. Being a release-time guard, it fails loudly rather
# than building the UI itself: `trunk` may not be installed, and quietly compiling wasm inside
# what looked like a packaging step is its own surprise.
set -eu

ROOT="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
DIST="$ROOT/crates/adi-webapp/dist"

[ -f "$DIST/index.html" ] || {
    echo "error: the webapp has not been built — $DIST/index.html is missing" >&2
    echo "       the control panel would ship with no UI at all" >&2
    echo "" >&2
    echo "       build it first:  (cd $ROOT/crates/adi-webapp && trunk build --release)" >&2
    echo "       or the lot:      $ROOT/scripts/build-app.sh" >&2
    exit 1
}

# An index.html with no wasm beside it means an interrupted or failed trunk run.
ls "$DIST"/*.wasm >/dev/null 2>&1 || {
    echo "error: $DIST has an index.html but no .wasm — the trunk build did not finish" >&2
    echo "       rebuild it:  (cd $ROOT/crates/adi-webapp && trunk build --release)" >&2
    exit 1
}
