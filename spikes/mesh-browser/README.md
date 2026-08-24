# Spike: a browser tab as its own iroh peer (ADI-13)

**Result: it works.** A headless Chrome tab, holding its own iroh identity, dialled a real
`adi-mesh` node over a relay, spoke `adi/mesh/http/1`, was checked against that node's grants and
its Basic-auth gate, and rendered the node's local service — **200 in 920 ms from a cold page
load**, of which ~600 ms is the relay session and ~150 ms the QUIC handshake.

This spike answers only the transport question ADI-13 stands on. There is no UI, no key store, no
service worker and no dashboard catalog here, on purpose: those are cheap once the transport is
real and worthless if it is not.

```
./run.sh                    # build both halves, run the node, run five browser cases, print verdicts
PROFILE=release ./run.sh    # …with the bundle built the way it would ship (also passes)
NODE_RELAY=<url> ./run.sh   # …with the node calling a different relay home (see finding 4)
```

Everything it starts, it stops. Re-runnable; each run mints a fresh browser identity.

## What is actually being run

| piece | what it is |
|---|---|
| `src/lib.rs` | the tab: an `iroh::Endpoint` on `wasm32-unknown-unknown`, one dial, one bi-stream |
| `src/protocol.rs` | **not a file** — `#[path]`-included from `crates/adi-mesh/src/protocol.rs` |
| the node | a real `target/debug/adi-mesh run`, against a scratch store at `~/.adi-mesh-spike/mono` |
| the relay | `mad.mono-relay.withadi.dev`, the fleet's own |
| the verdict | `report.json`, posted by the page to `serve.py` |

The node side is the production one — `gateway::serve_peer` → `admit` → `authenticated` — so the
refusals below are the real policy refusing, not a stub. Nothing touches `~/.adi/mono`; the scratch
store is separate through `ADI_DIR`, and the gateway's listener is moved to `127.0.0.1:45082`
because the live control panel holds the platform default (10080).

## The five cases, and what each proves

| # | case | result |
|---|---|---|
| 1 | granted service, right password | mesh `ok`, **HTTP 200**, the service's own bytes |
| 2 | granted service, wrong password | mesh `ok`, **HTTP 401** — the node's gate, on an accepted stream |
| 3 | a service the tab holds no grant for | **refused before HTTP**: "holds no grant for that service" |
| 4 | an identity the node never paired | **the same refusal as 3**, byte for byte |
| 5 | the tab's home relay is n0's, the node's is ours | **200**, through the node's relay |

3 and 4 answering identically is the non-enumerability property `gateway.rs::admit` exists for: an
unpaired key learns nothing about which services a node has.

## Four findings that change how the client gets built

**1. Nothing had to be ported.** iroh 1.0.3 compiles for `wasm32-unknown-unknown` untouched —
`wasm_browser` is a cfg *alias set by iroh's own build.rs*, not a flag anyone passes. `protocol.rs`
compiles as-is because it is generic over tokio's IO traits and noq's impls of those traits on QUIC
streams are not feature-gated. Bare `cargo build` was 56 s from cold.

**2. On a Mac you need Homebrew LLVM.** `ring` compiles C for wasm and Apple's clang has no wasm
backend: `unable to create target: 'No available targets are compatible with triple
"wasm32-unknown-unknown"'`. `.cargo/config.toml` here points `CC_wasm32-unknown-unknown` at
`/opt/homebrew/opt/llvm/bin/clang`. That is the entire host setup.

**3. Do not wait for a home relay.** `Endpoint::online()` waits for the relay the tab could be
*reached* through, and nothing ever dials a browser. In case 5 it never came up at all and the dial
still worked — it just cost the 8 s spent waiting. A dial-only client should skip it.

**4. The relay is a hard precondition, not a latency trade-off.** A browser has no UDP, so there is
no direct path to fall back to — and `https://euw1-1.relay.iroh.network` (n0's public relay, the
shipped default when `mesh.toml` names no relay) **will not accept a browser WebSocket at all**:

```
CloseEvent { code: 1006, reason: "", was_clean: false }
```

The cause is in the handshake, and `curl` shows it plainly: our relay answers `101` with
`sec-websocket-protocol: iroh-relay-v2` echoed back; that n0 relay answers `101` with no such
header, and RFC 6455 §4.1 requires a client that offered a subprotocol to fail a response that
does not name one. Consequence, measured with `NODE_RELAY=https://euw1-1.relay.iroh.network`:
**a browser cannot reach a node left on the shipped relay default** — the dial times out. Every
node a browser is meant to reach must call home to a browser-compatible relay. (Tested against one
n0 relay URL; the rest of their map is untested.)

## Bundle size, for a client served publicly

| | raw | gzip | brotli |
|---|---|---|---|
| release wasm | 3.49 MB | 1.37 MB | 0.98 MB |

Plus 46 KB of JS glue. Dev profile is 14.7 MB — build the shipped one with `--release`. This is a
bare dialer; the Leptos UI and the catalog are on top of it.

## What is still unproven

- **Long-lived streams.** One request/response was tested. WebSockets and SSE through the same
  splice — what a dashboard actually needs — were not.
- **The real fleet.** The node here is a real daemon but a scratch one. Dialling the operator's own
  node would mean writing this tab's key into the live `fleet.toml`, which is his call.
- **Mobile Safari.** Only headless Chrome on macOS was run.
- **A tab that is backgrounded**, which on iOS is what kills the iOS viewer's pooled connections
  (`adi-mesh-ffi/src/viewer.rs`, `STEP_TIMEOUT`). A browser tab will have its own version of that.
