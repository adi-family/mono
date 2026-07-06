#!/usr/bin/env bash
# Hot-reload dev loop for the adi webapp:
#   - an API backend (adi-app) on :8090
#   - Trunk's dev server on :9080 with browser auto-reload, proxying /api -> :8090
#
# Edit crates/adi-webapp/src/** or the adi-css SCSS and the browser refreshes itself; no
# binary swap, no root, no app.adi involved. Ctrl-C stops both.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$repo_root"

if ! command -v trunk >/dev/null 2>&1; then
  echo "error: 'trunk' is not installed. Install it with:  brew install trunk" >&2
  exit 1
fi

# Isolated dev registry so reserve/release from the panel don't touch real port state.
# Override by exporting ADI_DIR before running.
export ADI_DIR="${ADI_DIR:-.adi-dev}"

echo "==> API backend: adi-app on :8090  (ADI_DIR=$ADI_DIR)"
cargo run -q -p adi-app -- 8090 &
backend=$!
trap 'kill "$backend" 2>/dev/null || true' EXIT INT TERM

echo "==> UI: trunk serve on http://127.0.0.1:9080  (hot reload)"
cd crates/adi-webapp
exec trunk serve --open
