#!/usr/bin/env bash
# Build the pinned source snapshot, never a machine-local dashboard path.
set -euo pipefail
cd "$(dirname "$0")/../apps/primary-ui/packages/ui"
bun install --frozen-lockfile
bun run typecheck
bun run build
cd ../../frontend
bun install --frozen-lockfile
bun run typecheck
bun run test:primary
bun run test:home
bun run test:chat
bun run build:primary
