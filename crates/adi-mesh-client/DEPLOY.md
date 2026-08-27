# Deploying `mesh-client.withadi.dev`

The artefact is `crates/adi-mesh-client/dist/` — **static files, no server, no build step on the
host, no environment variables.** Everything the client shows it fetched itself over QUIC from a
machine that is listening on nothing, so there is no backend to point at and no secret to configure.

```
scripts/build-mesh-client.sh          # → crates/adi-mesh-client/dist/
```

| | |
| --- | --- |
| Build command | `scripts/build-mesh-client.sh` (or `cd crates/adi-mesh-client && trunk build --release`) |
| Output directory | `crates/adi-mesh-client/dist` |
| Environment variables | none |
| Size, as of 2026-08-27 | **3.55 MB raw / 1.06 MB brotli** wasm, plus 62 KB JS, 5 KB CSS and the icons — 3.6 MB on disk |

## Three requirements the host has to meet

**1. HTTPS, and a real certificate.** A service worker and `crypto.getRandomValues` need a *secure
context*. Without one the client cannot register its worker, so it cannot serve a panel at all — it
will render its own list and open nothing. (`http://127.0.0.1` and `http://localhost` are secure
contexts, which is how it is developed; no other plain-HTTP origin is.)

**2. `/sw.js` served from the root, uncached.** The worker's scope can only cover paths under its
own URL, so it has to sit at the origin root, and a cached copy is how a stale worker outlives a
deploy. Send `Cache-Control: no-cache` for that one file.

**3. A single-page fallback that does *not* swallow `/n/`.** The client is one page at `/`; any
other path should serve `/index.html`. `/n/<petname>/` is a real route it opens in an iframe, and
the service worker answers it before the network is consulted — but on the very first load, before
the worker is installed, that URL must not 404 the browser into a dead frame. Serving `index.html`
for it is correct and harmless.

## Cloudflare Pages — what to click

**This has not been done, and it cannot be done from this machine.** There are no Cloudflare
credentials in the secret store, `wrangler` is installed but unauthenticated, and connecting Pages
to a repository is a dashboard OAuth flow between Cloudflare and GitHub that no API token replaces
(the same wall `projects/adi-landing/DEPLOY.md` hit).

Dashboard → **Workers & Pages → Create → Pages**. Either route works:

*Direct upload* — fastest, and enough to try it on a phone today:

```
cd crates/adi-mesh-client
wrangler pages deploy dist --project-name adi-mesh-client
```

*Connect to Git* — the steady state, so a deploy is a push:

| field | value |
| --- | --- |
| Production branch | `main` |
| Build command | `scripts/build-mesh-client.sh` |
| Output directory | `crates/adi-mesh-client/dist` |
| Root directory | *(empty — the script cd's into the crate itself)* |

Pages' build image has no Rust toolchain, no `wasm32-unknown-unknown` target and no Homebrew LLVM,
so **a Git-connected build will not work as written**: `ring` compiles C for wasm and needs a clang
that has the backend. Either add a `rustup`/`apt` preamble to the build command, or build here and
use direct upload. Direct upload is the honest choice for now — the toolchain is the reason, not
laziness.

Add these to the project's **Headers** (Pages reads `dist/_headers`, which this build does not yet
write):

```
/sw.js
  Cache-Control: no-cache
/__adi/*
  Cache-Control: no-cache
```

Then attach `mesh-client.withadi.dev` in **Custom domains**.

## Verify, in this order

1. `curl -sI https://mesh-client.withadi.dev/sw.js | grep -i cache-control` — `no-cache`.
2. Open it on a phone. The page should show *"No machines yet"* and, at the bottom, **this
   browser's key** — a 64-character hex string. If the key is missing, IndexedDB is refusing the
   write (private browsing does this) and nothing else will work.
3. On a machine you want to reach: `adi-mono mesh invite`. Paste the token into the client and press
   **Pair**. It takes about five seconds — the client is waiting out the node's own registry reload
   (`docs/fleet.md` §8) so that the row, when it appears, is a row that opens.
4. Tap the machine. Its control panel should render.
5. Install it: Safari → Share → *Add to Home Screen*; Chrome → the install prompt.

## What the operator still owns

- **Every node a browser is meant to reach must be on a browser-compatible relay.** n0's public
  relay answers a websocket upgrade without echoing `sec-websocket-protocol`, RFC 6455 §4.1 makes
  the browser fail it, and the dial times out with nothing useful said. Ours works. This is the
  default **in source** as of `d37c607` but **is not deployed** (ADI-16) — a node running today's
  binary with no `relays` in its `mesh.toml` is unreachable from this client. Name it explicitly:

  ```toml
  # ~/.adi/mono/mesh/mesh.toml
  relays = ["https://mad.mono-relay.withadi.dev"]
  ```

- **The relay becomes bandwidth-bearing** (ADI-15). A browser has no UDP, so 100% of every session
  is relayed for its whole duration — including the panel's own multi-megabyte wasm bundle on first
  load. Today that is one host in Madrid.
