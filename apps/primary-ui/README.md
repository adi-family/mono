# Primary ADI interface

Release source snapshot of the design-system dashboard. SOURCE.json records the upstream
commit. Refresh frontend/ and packages/ui/ together from tracked dashboard sources; keep
frontend/src/lib/api.ts excluded (unused dashboard-only backend client).

Run `scripts/primary-ui.sh` from the repository root with Bun 1.3.5. The release workflow
builds this source once and transfers dist-primary to every platform before Rust compilation.
adi-app embeds the build, so installations need no dashboard checkout or environment override.
Marketplace catalog and package routes use React. Legacy Settings and embed routes keep using the native Leptos bundle.
ADI_PRIMARY_UI_DIST remains an optional disk override for local development.
