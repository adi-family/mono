# adi-webapp

The **adi control-panel UI** — a [Leptos](https://leptos.dev) app compiled to wasm. It's
the `app.adi` front end: summary tiles, a live port-registry table, and a reserve/release
form over the `/api/*` backend.

- Types come from [`adi-webapp-api`](../adi-webapp-api), so the client deserializes exactly
  the structs the server serializes.
- Styling comes from [`adi-css`](../adi-css) — the shared design system. The markup uses
  its `adi-*` classes; `styles/main.scss` just `@use`s the library, and Trunk compiles it
  into `<head>`.

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

## Notes

- Targets `wasm32-unknown-unknown`; **excluded from the workspace's `default-members`**, so
  a bare `cargo build`/`cargo test` skips it. Build with Trunk (or
  `cargo … -p adi-webapp --target wasm32-unknown-unknown`).
- No JavaScript, no npm — the toolchain is entirely Rust + Trunk.
