# adi-app

The **adi app** — the real thing served at `app.adi`. One small Rust process with two
roles:

- `GET /` (and any non-`/api` path) → the **control-panel UI**, the Leptos app
  [`adi-webapp`](../adi-webapp) compiled to wasm by Trunk and embedded here at build time
  (no runtime asset paths, no web server for statics).
- `/api/*` → the **Rust backend**: the handlers from
  [`adi-webapp-api`](../adi-webapp-api) (its `server` feature) over the live
  [`adi-ports-manager`](../adi-ports-manager) port registry.

adi-hive fronts it: `app.adi → 127.0.0.1:<port>`, and adi-hive's runner launches and
supervises this process, injecting `$PORT`. So the browser hits `app.adi`, the UI wasm
loads, and its `fetch('/api/...')` calls land on this same process through the proxy.

The `/api/*` request/response types are defined once in `adi-webapp-api` and shared by
both sides, so the wasm client deserializes exactly what this server serializes.

## API

| Method | Path                      | Does                                                        |
| ------ | ------------------------- | ----------------------------------------------------------- |
| GET    | `/api/health`             | `{ ok, service, version, uptime_secs }`                     |
| GET    | `/api/ports`              | `{ range, reserved[], leases[] }` — the allocator's state   |
| POST   | `/api/ports/reserve`      | body `{ service, key }` → reserves a static port, returns `{ port }` |
| POST   | `/api/ports/release`      | body `{ service, key }` → releases it, returns `{ freed }`  |

The reserve/release endpoints mutate the real registry at
`~/.adi/mono/ports/registry.json` (honoring `$ADI_DIR`), so the panel is operating
actual platform state, not a mock.

## Build

The UI must be built before adi-app, because adi-app embeds its `dist/`. The one-shot
script does both:

```sh
scripts/build-app.sh          # trunk build --release, then cargo build --release -p adi-app
scripts/build-app.sh --debug  # debug profile
```

By hand:

```sh
( cd ../adi-webapp && trunk build --release )   # produces crates/adi-webapp/dist/
cargo build -p adi-app                          # embeds it
```

A fresh checkout still compiles before the UI is built: a `build.rs` creates an empty
`dist/` and adi-app serves a placeholder page (styled with [`adi-css`](../adi-css)) until
Trunk populates it.

## Dev mode

For fast UI iteration, prefer the webapp's hot-reload loop (`scripts/dev.sh`). To iterate
against **this** server (or app.adi itself) without re-embedding, set `ADI_WEBAPP_DIST` to
a `dist/` directory — adi-app then serves the UI from disk instead of the embedded copy, so
a plain `trunk build` (or `trunk watch`) updates the page on the next refresh:

```sh
ADI_WEBAPP_DIST="$PWD/../adi-webapp/dist" adi-app 8090
```

To turn the real **app.adi** into a live-updating target, set `ADI_WEBAPP_DIST` on the
front-door's `app` runner (a one-time change + restart); after that, `trunk build` alone
refreshes the UI — no re-embed, no binary swap.

## Primary interface

Set `ADI_PRIMARY_UI_DIST` to a built React interface to make it the main app while retaining
the original control panel. This is separate from `ADI_WEBAPP_DIST`, which continues to
override only the original Leptos build.

Build the native API variant, which writes `dist-primary` and sends requests directly to
`/api` on the panel's own origin:

```sh
cd /path/to/design-system/frontend
bun run build:primary
ADI_PRIMARY_UI_DIST="$PWD/dist-primary" adi-app
```

The primary build serves `/`, `/index.html`, `/interface`, `/library`,
`/library/agents/layout-reviewer`, and `/chat` or `/chat/*` compatibility bookmarks. Its
existing asset files are served from the same origin. `/extended/*` (including Settings),
`/marketplace/*`, `/embed/*`, and other routes or assets continue to use the original panel.
The native API remains at `/api/*`.

The directory must contain a readable `index.html`. An invalid setting logs a warning and
leaves the original UI working. Missing primary files also fall back to the original panel;
asset paths and symlinks cannot escape the configured distribution directory. Build first,
then set the variable on the existing managed `adi-app` service and restart only that service.

## Run

```sh
adi-app            # listens on 127.0.0.1:$PORT (else :8090)
adi-app 8091       # explicit port
adi-app 127.0.0.1:8091
```

Under adi-hive it needs no arguments — the runner injects `$PORT`. See
[`hive.example.yaml`](./hive.example.yaml) for the `app.adi` wiring.

## Design

Hand-rolled HTTP/1.1 (`src/http.rs`), the same dependency-light approach as adi-hive's
proxy — no web framework. Every response is `Connection: close`, so each request is its
own connection and there's no keep-alive framing to track. `src/main.rs` routes `/api/*`
to `adi_webapp_api::handlers` and serves everything else out of the webapp — the embedded
`include_dir!` copy by default, or a disk `dist/` when `ADI_WEBAPP_DIST` is set — falling
back to the app shell for unknown paths so the client-side router can resolve them.

Release builds also embed the primary React interface from `apps/primary-ui/frontend/dist-primary`.
Build it with `scripts/primary-ui.sh` before packaging. CI builds it once and downloads the
same artifact on all platforms; release compilation refuses a missing primary shell.
`ADI_PRIMARY_UI_DIST` is optional and overrides that embedded interface for development.
