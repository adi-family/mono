#!/usr/bin/env bash
#
# Point this clone's git hooks at the ones tracked in the repo.
#
# `.git/hooks` is per-clone and never travels with a checkout, so the hooks live in `.githooks/`
# and git is told to look there. One command, idempotent, no symlinks to go stale.
#
#   scripts/install-hooks.sh
#
# To go back to git's default: `git config --unset core.hooksPath`.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

chmod +x .githooks/* 2>/dev/null || true
git config core.hooksPath .githooks

echo "hooks installed: core.hooksPath = .githooks"
for hook in .githooks/*; do
    [ -f "$hook" ] || continue
    echo "  $(basename "$hook")"
done
