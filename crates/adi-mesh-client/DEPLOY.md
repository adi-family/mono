# Deploying `mono-mesh-client.withadi.dev`

> **Live 2026-08-27** on both <https://mono-mesh-client.withadi.dev> and
> <https://mono-mesh-client.pages.dev>. Cloudflare Pages project **`mono-mesh-client`**; the DNS
> record was added by the operator and the certificate issued.
>
> **One thing is still wrong on the custom domain**, and it is a zone setting rather than anything
> in this build — see *[The zone overrides `_headers`](#the-zone-overrides-_headers-on-the-custom-domain)*.

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
| Size, as of 2026-09-02 | **3.66 MB raw / 1.09 MB brotli** wasm, plus 64 KB JS, 20 KB CSS (the shell, the shared tokens, the font rules), the icons and 280 KB of fonts — 4.0 MB on disk. A phone fetches only the Geist subsets its text uses, ~50 KB for Latin; Bricolage is in the directory because `adi-ui/fonts` is copied whole, and no rule here ever asks for it |

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

**`wrangler` is authenticated on this machine** — an OAuth token for `ihor@withadi.dev`, account
`5b81c76ca545338aa9e85215c001a768`, carrying `pages (write)`. (An earlier draft of this file said
deploying was impossible from here, which was true when `projects/adi-landing/DEPLOY.md` hit the
same wall and is no longer true.) So a deploy is two commands:

```
scripts/build-mesh-client.sh
cd crates/adi-mesh-client
wrangler pages deploy dist --project-name mono-mesh-client --branch main
```

`--branch main` is what makes it a *production* deploy rather than a preview; without it the build
lands on a preview URL and the custom domain keeps serving the previous one.

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

Requirements 2 and 3 are **no longer manual**: the build now writes `dist/_headers` and
`dist/_redirects`, and Pages reads both (it names them as it uploads). They live at the crate root
and are copied by the `copy-file` links in `index.html`, so every build carries them — nothing has
to be re-entered in the dashboard after a deploy.

## The zone overrides `_headers` on the custom domain

**Measured 2026-08-27, on the same deployment, one minute apart:**

| | `/sw.js` | `/__adi/panel-shim.js` |
| --- | --- | --- |
| `mono-mesh-client.pages.dev` | `cache-control: no-cache` | `no-cache` |
| `mono-mesh-client.withadi.dev` | **`max-age=14400`** | **`max-age=14400`** |

Same files, same build, same `_headers`. The difference is that the custom domain is **proxied
through the `withadi.dev` zone** and `pages.dev` is not, so the zone's *Browser Cache TTL* — four
hours, Cloudflare's long-standing default — is applied on top of what Pages sends. `_headers` is
not being ignored; it is being overridden downstream.

Why it matters, and why it is not a crisis: browsers largely bypass the HTTP cache when they check
a **service worker** script for updates (`updateViaCache` defaults to `"imports"`), so `sw.js`
mostly escapes. `/__adi/panel-shim.js` does not — it is an ordinary subresource, and it is injected
into every panel by the worker. The two are written to move together; a four-hour window where a
new worker meets an old shim is exactly what the `_headers` rule exists to prevent.

**The fix is a zone setting and needs a browser** — the wrangler OAuth token cannot even *read*
zone settings (`10000 Authentication error` on `GET /zones/<zone>/settings/browser_cache_ttl`), let
alone write them:

- *withadi.dev* → **Caching** → **Configuration** → **Browser Cache TTL** → **Respect Existing
  Headers**. Zone-wide, one setting, and it is the correct posture for a zone that serves builds
  whose filenames already carry a content hash.
- Or, if something else on the zone wants that TTL, a **Cache Rule** matching
  `http.host eq "mono-mesh-client.withadi.dev" and (http.request.uri.path eq "/sw.js" or
  http.request.uri.path contains "/__adi/")` with *Browser TTL → Respect origin*.

Verify with the table above: both rows should read `no-cache`.

## Attaching the custom domain — done, and what it took

Attaching the custom domain is two separate things, and only the first one worked from here.

**Done** — the domain is registered against the Pages project. `wrangler` has no command for it, so
it goes through the API with the token wrangler already holds:

```
POST /client/v4/accounts/<account>/pages/projects/mono-mesh-client/domains
     {"name":"mono-mesh-client.withadi.dev"}          → success, status "initializing"
```

**Needed a human** — the `CNAME`. (Added by the operator on 2026-08-27; the domain went `active`
and the certificate issued. Recorded because the next deploy to a new hostname will hit it again.)
The zone `withadi.dev` (`7dcdc690ed524a9d1ede599897066c0f`) is on the same account and active, but
**Pages did not create the record**, and creating it by hand fails:

```
POST /client/v4/zones/<zone>/dns_records   → 10000 Authentication error
```

The wrangler OAuth token carries **`zone (read)`**, not DNS edit, and no `pages (write)` call
creates a record on your behalf with it. Until the record exists the custom domain stays `pending`,
no certificate is issued, and the name does not resolve at all. **This is the one step that needs a
human at a browser** — and the reason to write it down is that it looks like a Pages problem and is
not one.

Either fix works:

- **Dashboard** → *Workers & Pages* → `mono-mesh-client` → *Custom domains* → *Set up a custom
  domain*. A dashboard session has the permission the token lacks and creates both halves at once.
- **Or** the record by hand: *withadi.dev* → *DNS* → add `CNAME`, name `mono-mesh-client`, target
  `mono-mesh-client.pages.dev`, **proxied**. The Pages side is already waiting for it and flips to
  `active` on its own, usually within a minute or two.
- **Or**, to make this scriptable next time, mint an API token with *Zone → DNS → Edit* on
  `withadi.dev`, store it as `adi-mono secrets set CLOUDFLARE_DNS_TOKEN`, and the whole deploy
  becomes unattended.

## Verify, in this order

0. Until the DNS record above exists, verify against `https://mono-mesh-client.pages.dev` — the
   deploy is complete and the client fully works there; only the vanity name is missing.
1. `curl -sI https://mono-mesh-client.withadi.dev/sw.js | grep -i cache-control` — `no-cache`. Check
   `content-type` is `application/javascript` too: if it comes back `text/html`, the `/*` rule in
   `_redirects` has swallowed a real asset and the worker will never install.
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
