# adi-mesh-client — a browser tab that is its own iroh peer

Open it on a phone, pair it with your machines, and open each one's control panel — and whatever
else that machine runs. There is no server: the page is static files, and everything it shows it
fetched itself over QUIC from a machine that is listening on nothing — no `:22`, no `:80`, no
`:443`.

`docs/fleet.md` §12 is the contract. This file is how it is built, how it is tested, and the three
things that cost a day to find out.

```
scripts/build-mesh-client.sh          # → dist/, the deployable artefact
harness/run.sh                        # the long-lived-stream measurement, against a real node
harness/e2e.sh                        # pair → list → open → use, driven in a real browser
harness/e2e.sh --scan                 # the same, with the invite arriving through a camera
```

## What is in here

| | |
| --- | --- |
| `src/mesh.rs` | an `iroh::Endpoint` on wasm32: dial-only, pooled per node, relay-pinned |
| `src/protocol.rs` | **not a file** — `#[path]`-included from `crates/adi-mesh/src/protocol.rs` |
| `src/http.rs` | HTTP/1.1 as a client, with the body delivered as chunks rather than read to end |
| `src/ws.rs` | an RFC 6455 client, spoken by the tab over the same QUIC stream |
| `src/invite.rs` | pairing, from the side that dials (`docs/fleet.md` §8) |
| `src/scan.rs` | the camera, and reading an invite out of it — the other end of `mesh invite --qr` |
| `src/store.rs` | IndexedDB: the browser's secret key, and one record per node |
| `src/dashboards.rs` | what a node runs, asked of the node's own panel — and asking for a grant |
| `src/bridge.rs` | answering the service worker's `fetch` and a panel's `new WebSocket()` |
| `src/ui.rs` | the shell: the machines, what each runs, and the button that adds one |
| `src/mark.rs` | the adi mark, with the geometry pinned to the tree's three other copies |
| `sw.js` | which node *and service* a request belongs to, and how a whole app is served from one |
| `js/panel-shim.js` | injected into a panel's `<head>`: gives it back `/`, and re-points its sockets |
| `src/probe.rs` | the measurement the design stands on, kept as a regression test |

## How a node's control panel becomes a page here

A panel is a whole single-page app that fetches everything with **root-absolute** URLs, because
§4 requires that a dashboard never learn its own address. So it cannot be mounted under a path, and
one origin cannot host two of them by URL. The client id does it instead:

1. The shell opens an iframe at `/n/<petname>/` — or `/n/<service>.<petname>/` for one of the
   dashboards that machine runs, which is the same mechanism with the service named.
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

**A second service on the same machine**, in the same run (measured 2026-08-27): the machine's row
listed `Probe` at **3.0 s**, tagged `Allow` because pairing granted only `http:app`; tapping it
asked the node for `http:probe`, and the dashboard rendered at **7.5 s** — the four seconds between
are the node's own reload of `fleet.toml`, waited out rather than raced. The grant is written into
the real registry by the fixture standing in for the panel (`harness/upstream.py`), so what admits
the second dial is the node.

**Pairing by camera**, `harness/e2e.sh --scan`, same run with nobody typing anything. The QR that
`adi-mono mesh invite --qr` printed is rendered into a video (`harness/qr-y4m.py`) and handed to
Chrome *as its camera*; the page is never told the token. Measured 2026-08-27: Scan pressed at
**203 ms**, camera open at **512 ms**, paired and listed at **2.6 s** — and afterwards every track
the scanner opened reads `ended`, which is asserted rather than eyeballed.

## The scanner, and what is not proven about it

Pairing means getting a **953-character** token onto a phone, so `src/scan.rs` reads it off the
screen instead. The paste field stays exactly as it was: the camera is an accelerator on a flow
that has to keep working when it is refused, absent, or pointed at nothing.

- **The decoder is `quircs`, not `BarcodeDetector`.** That API is Chromium-only — a scanner built
  on it is a button that does nothing on an iPhone, which is the device this whole client is for.
- **Not `rqrr` either**, the other pure-Rust reader: measured on this bundle it costs 92 KB brotli
  against quircs' 22 KB, and what the extra buys is adaptive rather than whole-frame thresholding,
  which matters most on paper under a shadow. This one is pointed at a lit screen. `scan.rs`'s unit
  tests hold it to a photograph of a screen lit from one side, at four pixels a module.
- **The Scan button is not drawn where `navigator.mediaDevices` does not exist** (an insecure
  origin). Present-and-inert would be worse than absent.

**What has not been tested: a real iPhone, in Safari or as an installed home-screen app.** There is
no device on the machine this was built on. What was done instead: the whole flow in headless
Chrome through a fake camera (above); the decoder itself against synthetic photographs, natively;
and the printed QR decoded by **Apple's own Vision framework**, which is the detector iOS's Camera
app uses — so what the CLI draws is known to be readable by the reader that matters, even though
the browser holding the camera has not been the one on an iPhone. The iOS-specific hazards are
handled in code and commented where they are (`playsinline`, `muted` as a property and not an
attribute, `facingMode: {ideal}` rather than `exact`), but they are reasoned, not observed. A
refusal inside an installed app is remembered by iOS with no second prompt — which is the reason
every failure message ends by pointing at the paste field.

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

## What a machine runs, and the origin it is rendered on

Under each machine in the list are the dashboards it runs, read from that node's own
`GET /api/dashboards` over `app` — the service pairing grants — with what this browser may reach
read from its `GET /api/fleet` **by key**. That is `docs/fleet.md` §11's fleet rail asked from a
phone, and it needs no new wire format: `adi/mesh/http/1` deliberately cannot answer "what do you
serve?", because §5 refuses an unauthorized peer before the route table is consulted.

Pairing grants `http:app` and nothing else, so a row is usually one this browser cannot open yet. It
says `Allow`, and the first tap asks the node for `http:<service>` and then waits out the node's
five-second registry reload — the same wait a pairing pays. Nothing here decides anything: the node
writes the grant, and the node enforces it.

**The cost is one origin.** This origin's IndexedDB holds the browser's secret key *and* every
node's password, so a dashboard rendered here can read the credentials for every machine in the
list. What bounds it is that the code is the operator's own and that nothing is reached that a tap
did not ask a node to allow — not isolation, which is `docs/fleet.md` I8: a sub-origin per
`(node, service)` under a wildcard domain, with the key store on the parent. Still open.

## What this still deliberately does not do

- **It dials and never hosts.** No ALPN is registered on the endpoint. Nothing can reach this tab.
- **It is a reader, not a machine in the fleet.** Close the tab and every panel in it goes with it.
