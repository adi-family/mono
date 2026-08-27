# adi-mesh-client — a browser tab that is its own iroh peer

Open it on a phone, pair it with your machines, and open each one's control panel. There is no
server: the page is static files, and everything it shows it fetched itself over QUIC from a machine
that is listening on nothing — no `:22`, no `:80`, no `:443`.

`docs/fleet.md` §12 is the contract. This file is how it is built, how it is tested, and the three
things that cost a day to find out.

```
scripts/build-mesh-client.sh          # → dist/, the deployable artefact
harness/run.sh                        # the long-lived-stream measurement, against a real node
harness/e2e.sh                        # pair → list → open → use, driven in a real browser
```

## What is in here

| | |
| --- | --- |
| `src/mesh.rs` | an `iroh::Endpoint` on wasm32: dial-only, pooled per node, relay-pinned |
| `src/protocol.rs` | **not a file** — `#[path]`-included from `crates/adi-mesh/src/protocol.rs` |
| `src/http.rs` | HTTP/1.1 as a client, with the body delivered as chunks rather than read to end |
| `src/ws.rs` | an RFC 6455 client, spoken by the tab over the same QUIC stream |
| `src/invite.rs` | pairing, from the side that dials (`docs/fleet.md` §8) |
| `src/store.rs` | IndexedDB: the browser's secret key, and one record per node |
| `src/bridge.rs` | answering the service worker's `fetch` and a panel's `new WebSocket()` |
| `src/ui.rs` | the shell: a list, a text box, and an iframe |
| `sw.js` | which node a request belongs to, and how a whole app is served from one |
| `js/panel-shim.js` | injected into a panel's `<head>`: gives it back `/`, and re-points its sockets |
| `src/probe.rs` | the measurement the design stands on, kept as a regression test |

## How a node's control panel becomes a page here

A panel is a whole single-page app that fetches everything with **root-absolute** URLs, because
§4 requires that a dashboard never learn its own address. So it cannot be mounted under a path, and
one origin cannot host two of them by URL. The client id does it instead:

1. The shell opens an iframe at `/n/<petname>/`.
2. The service worker sees that navigation, records `resultingClientId → petname`, and answers it
   from the node — with `js/panel-shim.js` injected as the first thing in its `<head>`.
3. The shim rewrites the iframe's URL to `/`, so the panel's own router, its `pushState` and the
   back button behave exactly as they do at `app.adi`.
4. Every later request from that client id — `/api/health`, a wasm chunk, a font — goes to the same
   node, whatever its path. `/__adi/` is the one reserved exception, because that is where the shim
   itself lives and a panel has to be able to load it.

`new WebSocket()` is the one thing no service worker can see — it is not a `fetch`, and no `fetch`
event ever fires for it. The shim replaces the constructor with one that asks the shell, and the
shell speaks RFC 6455 itself over a mesh stream.

## The measurements

**Long-lived streams cross `adi/mesh/http/1` from a tab.** This was the open question the whole
design stood on, and `harness/run.sh` answers it against a real `adi-mesh` daemon over
`mad.mono-relay.withadi.dev`. Measured 2026-08-27:

| case | result |
| --- | --- |
| one request, one response | HTTP 200, ~55 ms once the stream is admitted |
| an event stream | 5 events **spread over 5146 ms** — pumped, not buffered |
| a websocket, spoken by the tab | `101` verified against its `Sec-WebSocket-Accept`; echo round trip **~55 ms**; an unprompted server push arrived |

The spread is the whole verdict. A node that buffered would deliver the same bytes — it would just
deliver them all at once, at the close.

**The client end to end**, `harness/e2e.sh`, in headless Chrome against a real node, driving the
real UI: pair from an `adi-invite:` in **2.5 s**, panel rendered at **2.8 s**, the panel's own
root-absolute `/api` answered at **3.0 s**, its own `new WebSocket()` answered in the same tick.
With `APP_PORT=8000 REAL=1` — pointing the node's `app` service at a real `adi-app` — the actual
control panel rendered through the client in **7.5 s** from a cold page load.

## Three things that cost a day

**1. `cargo build -p adi-mesh-client` from the repo root fails in `ring`.** Cargo reads
`.cargo/config.toml` from the *current directory* upward, and the file that points cc-rs at
Homebrew's LLVM is in this crate. Apple's clang has no wasm backend and `ring` compiles C for every
target it supports. Build from here, or through `scripts/build-mesh-client.sh`, which cd's first.

**2. A pairing is not usable the instant it is accepted.** The node's gateway serves from an
in-memory snapshot of `fleet.toml` and re-reads it every five seconds. The join handshake writes the
file and answers immediately, so the first request lands on a registry that has never heard of the
new key and is refused with the sentence a *stranger* gets. `invite::wait_until_admitted` waits it
out, which is why **Pair** takes about five seconds and why the row, when it appears, opens.

**3. `cargo fmt --all` reformats about 200 files in this workspace.** The tree is not fmt-clean at
`HEAD`. Format this crate alone: `cargo fmt -p adi-mesh-client`.

## What v1 deliberately does not do

- **One service: the node's own panel.** `http:app`, the default grant. This origin's IndexedDB
  holds the browser's secret key *and* every node's password, so anything rendered here can read the
  credentials for every machine in the list. Arbitrary dashboards want a sub-origin each, under a
  wildcard domain, with the key store on the parent — `docs/fleet.md` I8.
- **It dials and never hosts.** No ALPN is registered on the endpoint. Nothing can reach this tab.
- **It is a reader, not a machine in the fleet.** Close the tab and every panel in it goes with it.
