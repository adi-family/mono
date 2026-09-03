# adi-webapp

The **adi control-panel UI** — a [Leptos](https://leptos.dev) app compiled to wasm. It's
the `app.adi` front end: summary tiles, a live port-registry table, and a reserve/release
form over the `/api/*` backend.

- Types come from [`adi-webapp-api`](../adi-webapp-api), so the client deserializes exactly
  the structs the server serializes.
- Styling is [`design/DESIGN.md`](../../design/DESIGN.md), drawn from one token file:
  [`design/tokens.css`](../../design/tokens.css). Two stylesheets both resolve to it —
  [`adi-css`](../adi-css)'s `adi-*` classes (`styles/main.scss` `@use`s the library and adds
  the shell's own shapes; one partial per page under `styles/pages/`) and
  [`adi-ui`](../adi-ui)'s Tailwind utilities (`styles/tailwind.css`). Trunk compiles both into
  `<head>`. The app is dark; there is no theme toggle.

## Hot-reload dev loop (recommended)

```sh
scripts/dev.sh          # API backend on :8090 + trunk serve on :9080 (auto-reload)
```

Edit `src/**` or the adi-css SCSS → the browser refreshes itself (~1s; CSS-only edits are
near-instant). No binary swap, no root, no app.adi involved. Ctrl-C stops both. Under the
hood it runs a dev `adi-app` for `/api` and `trunk serve` (which proxies `/api` to it — see
[`Trunk.toml`](./Trunk.toml)). For the styling workflow, see
[adi-css → Working on styles](../adi-css/README.md#working-on-styles).

## Production build

[Trunk](https://trunkrs.dev) compiles this crate to wasm and writes the bundle to `dist/`,
which [`adi-app`](../adi-app) embeds at build time:

```sh
scripts/build-app.sh    # trunk build --release, then cargo build -p adi-app
```

`dist/` is **not** committed. A fresh checkout still compiles adi-app before the UI is
built (it serves a placeholder until Trunk populates `dist/`).

## Installable app (PWA)

The panel installs as a standalone desktop/mobile app. Four pieces, all copied into `dist/`
verbatim (`rel="copy-file"` / `copy-dir`, so their URLs stay stable and unhashed):

| File | Role |
| --- | --- |
| [`manifest.webmanifest`](./manifest.webmanifest) | name, icons, `display: standalone`, shortcuts |
| [`sw.js`](./sw.js) | service worker: caches the shell, **never** `/api/*` |
| [`assets/`](./assets) | icons — `any` + `maskable` at 192/512, apple-touch, favicon |
| the inline script in [`index.html`](./index.html) | registers the worker, parks `beforeinstallprompt` on `window.__adiPwa` |

[`src/pwa.rs`](./src/pwa.rs) is the Rust side of that last bridge: it drives the **install
button** in the root bar and the workbench titlebar. The button only renders while the
browser actually has an install to offer, so it self-hides once installed.

### ⚠️ Install from `http://localhost:8000`, not `http://app.adi`

Service workers — and therefore installing — require a **secure context**. A loopback origin
counts as one; `http://app.adi` does not, and the front door speaks plain HTTP. So on
`app.adi` the browser withholds `navigator.serviceWorker` entirely, no install event fires,
and the install button stays hidden (verified: manifest still parses, nothing errors — it
just degrades). Open **`http://localhost:8000`** to install. Making `app.adi` installable
means giving the front door TLS.

The worker caches the shell only. Offline you get the frame and honest failures from the API
calls inside it — a stale port table or agent list would be worse than an error. Bump `CACHE`
in `sw.js` to retire every cached file at once.

Icons are derived from the macOS app icon, [`apps/macos/ADI.icns`](../../apps/macos/ADI.icns).
The `maskable` variants are full-bleed (the plate scaled until its rounded corners reach the
edge, over its own edge colour) so a platform mask has no transparent gaps to show; the glyph
stays within the 80%-diameter safe zone.

## Notes

- Targets `wasm32-unknown-unknown`; **excluded from the workspace's `default-members`**, so
  a bare `cargo build`/`cargo test` skips it. Build with Trunk (or
  `cargo … -p adi-webapp --target wasm32-unknown-unknown`).
- No npm, and the UI itself is Rust end to end. The only JavaScript is the PWA plumbing
  above — a service worker and the `beforeinstallprompt` bootstrap, neither of which can live
  in wasm.
