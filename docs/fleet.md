# Fleet — remote adi nodes over the mesh

A **node** is a full adi machine (hive + dns + app + dashboards) whose only network-facing
action is an *outbound* QUIC session to an iroh relay. It listens on nothing but loopback:
no `:22`, no `:80`, no `:443`. You reach it from your own machine through the mesh, and its
services appear in your browser under real hostnames.

This document is the contract. Everything below is normative; the checklist at the end is
what has to be built.

## 1. The namespace

```
<service>.<node>.n.adi
```

`n.adi` is **reserved**: any hostname ending in it addresses a remote node, never a local
service. Local services keep their existing `<service>.adi`, so the two can never collide.

- `nosh.laptop-b.n.adi` — the `nosh` dashboard on node `laptop-b`
- `app.laptop-b.n.adi` — that node's control panel (adi-app)
- On the node itself the same service is also reachable locally as `nosh.adi`.

`adi-dns` already resolves this: it suffix-matches with `zone_of` (`adi-dns/src/main.rs:206`),
so every `*.adi` name of any depth lands on the front door. No DNS change is required for
resolution — the work is in the front door and the gateway.

## 2. Names: nickname, petname, key

Three roles, never conflated:

| Role         | What it is                              | Scope             |
| ------------ | --------------------------------------- | ----------------- |
| **Key**      | the node's `EndpointId` (Ed25519)       | global, true name |
| **Nickname** | what the node calls itself              | a *suggestion*    |
| **Petname**  | what *this* machine calls it            | local, unique     |

Rules:

1. A name is one DNS label: `^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$`, lowercase.
2. At pairing the node offers its nickname. If free in the local registry it becomes the
   petname; if taken, the operator picks another (or we suggest `<name>-2`).
3. **A name collision never refuses the connection.** Authorization is by key; the name is
   presentation. Refusing to pair over a cosmetic clash is a bug.
4. The petname→key binding is **pinned** at pairing (TOFU). If the node later re-declares a
   different nickname, that is a *notification*, never a silent re-point — otherwise any node
   could rename itself to `main` and hijack links.
5. The petname is renameable **locally**, without the far side's involvement. This is the
   escape hatch when you belong to two fleets that both use `main`.
6. The key is the identity of record. It is shown at pairing and on the not-paired error page,
   so an operator has something to act on. It never appears in everyday URLs.

Within one organisation names are agreed socially, like ordinary hostnames — that is the
intended mode. The petname layer only exists so cross-fleet collisions stay solvable.

## 3. Routing

```
browser → local adi-hive (front door)
        → local mesh gateway (loopback)
        → [iroh, ALPN adi/mesh/http/1] → node's mesh gateway
        → node's local service port
```

- The front door routes any `*.n.adi` host to the local gateway's loopback port. One rule
  covers every node; there is no per-node route and no per-service local port.
- The gateway parses the host into `(service, node)`, resolves `node` → key through the local
  fleet registry, and opens one bi-stream on a **pooled** connection to that peer.
- **Over the mesh the route is the peer key plus the service label.** The host suffix is not
  used for routing on the far side, because the node cannot know what *you* call it. This is
  also why the `Host` header is never rewritten: rewriting it would send the node's absolute
  redirects to a same-named host on the viewer's machine.
- Unknown node (not in the registry, or paired but unreachable) → a dedicated error page
  naming the node and, when known, its key: *"node `x` is not paired with this machine — ask
  its administrator to pair key `…`"*. This is distinct from the existing 404 and 502 pages.

## 4. One origin per dashboard

A dashboard is **one** hostname. Its backend lives under `/api` on that same host, served
through hive path routing (the `proxy.path` field already exists in the schema and is
currently unused, `adi-hive/src/config.rs:90`).

The page must therefore use **relative URLs only**. It must never learn its own address.
This is what makes the same dashboard work under `nosh.adi`, `nosh.laptop-b.n.adi` and a real
customer domain later, for every viewer, with no substitution and no cookies.

The current scaffold violates this: `frontend/index.ts` reads the backend's leased port out of
`ports/registry.json` and injects `backendPort` into the HTML, so the browser talks to
`127.0.0.1:<port>` directly. Over the mesh that address is the *viewer's* machine. This must go.

## 5. Access control

Two independent layers. Both are required.

**Mesh layer (machine-to-machine).** Per-peer grants, **default-deny**. Today an empty
`authorized_peers` means *any peer* (`adi-mesh/src/host.rs:103`); on an endpoint reachable
through a public relay that is a footgun and is fixed here. A grant names what the peer may
reach (`http:*`, `http:app`, `tcp:127.0.0.1:22`, `ctl:read`). Unknown keys are rejected at the
iroh `after_handshake` hook, before any stream is opened.

**HTTP layer (human-to-service).** Basic authentication, enforced **on the node**, for
everything reachable over the mesh — including `app.<node>.n.adi`. The mesh grant is
machine-scoped: once your laptop is paired, *any process on it* can reach the node through your
front door. The password is what stops that, and what stops another person on a shared machine
from modifying the node.

- `401` with `WWW-Authenticate: Basic realm="<node>"` when absent or wrong.
- Credentials per node, stored hashed (salted SHA-256), compared in constant time.
- The check happens before the request reaches any service, and applies to WebSocket upgrades
  too — a gate that only covers plain requests is not a gate.
- Fail-closed: a node refuses to bind a non-loopback address unless credentials are configured.

## 6. What runs on a node

`adi-mono`, `adi-hive`, `adi-dns`, `adi-app`, and the mesh daemon. The node keeps its own hive
(so a real domain can be attached to a service later) and its own DNS (so its services resolve
each other locally). Supervision is systemd: `adi-core`'s supervisor abstraction now has a
`systemd --user` back-end beside the launchd and Task Scheduler ones (`adi-core/src/launchd.rs`).
A node's units live in `~/.config/systemd/user`, and lingering is enabled so they survive the
installing session logging out — without it a headless node stops the moment you disconnect.

**Two front-door configs exist, and a node has only one of them.** A developer machine carries a
hand-managed `~/.adi/mono/hive/hive.yaml` — the richer one, which imports every project and
dashboard. A freshly installed node has only the config `adi-core` generates,
`~/.adi/mono/dns/hive-frontdoor.yaml`. Anything resolving a service label must therefore prefer
the hand-managed file *and fall back* to the generated one; reading only the canonical path leaves
a node with an empty route table, and it then refuses every request with `ServiceUnknown` while
looking entirely healthy — paired, authorized, reachable, serving nothing. This was invisible on
a developer machine and total on a real node; it is pinned by
`gateway::tests::the_route_table_follows_the_front_door_a_node_actually_runs`.

## 7. Wire protocol — `adi/mesh/http/1`

One bi-stream is one HTTP connection. Connections to a peer are **pooled**: one iroh
`Connection` per peer, one bi-stream per HTTP connection. (Today a fresh iroh connection is
dialled per TCP accept — `adi-mesh/src/client.rs:7` — which would mean a handshake per request.)

Request header, then the raw HTTP bytes:

```
[version: u8 = 1][service_len: u8][service: utf-8, 1..=63 bytes]
```

Reply: one status byte, then the raw HTTP bytes on `Ok`.

| Byte | Status                | Meaning                                            |
| ---- | --------------------- | -------------------------------------------------- |
| 0    | `Ok`                  | resolved and connected; HTTP follows                |
| 1    | `ServiceUnknown`      | no such service label on this node                  |
| 2    | `NotAuthorized`       | the peer holds no grant for this service            |
| 3    | `UpstreamUnavailable` | the service is known but nothing is listening       |

HTTP-level failures (`401`, `502`) are ordinary HTTP responses on an `Ok` stream. Transport
failures are the status byte, so the caller can render a precise local error page.

The existing raw-TCP forward keeps its own ALPN (`adi/mesh/forward/0`) for ssh, databases and
anything not HTTP. iroh's `protocol::Router` accepts several ALPNs on one endpoint.

## 8. Pairing — `adi/mesh/join/1`

Pairing is what makes §6's "no inbound anything" real: **the node dials out**. It never needs an
open port, and the operator never needs ssh reachable to enrol it — a cloud-init line is enough.

The viewer mints an invite:

```
adi-invite:<hex(json({ v:1, endpoint:"adimesh:…", nonce:"<32 hex>", expires:<unix> }))>
```

One bi-stream on the join ALPN, both frames `[len: u32 BE][json]`, capped at 64 KiB:

```
node   → viewer   { "v":1, "nonce":"…", "nickname":"laptop-b" }
viewer → node     { "result":"accepted", "petname":"laptop-b", "username":"adi",
                    "password":"…", "grants":["http:app"] }
       or         { "result":"refused",  "reason":"…" }
```

- **The key never appears in a payload.** It is `Connection::remote_id()`, authenticated by the
  QUIC handshake. A key read out of JSON would be a claim, not a fact.
- **A nonce is single-use.** The viewer records every minted nonce and stamps it spent *before*
  the reply goes out, so a replayed invite cannot enrol a second machine. Expiry is checked
  before spentness, and pruning only ever moves a nonce from *expired* to *unknown* — never back
  to usable.
- **A name clash still pairs** (§2 rule 3): the viewer answers with a free suggestion, and the
  node is enrolled under it.
- **The password is generated by the viewer, stored as a verifier on both sides, and printed
  once** — on the node, where the operator ran `join`. It is persisted nowhere in plaintext; the
  browser asks for it. Re-pairing a known key keeps the pinned petname and rotates the password,
  because a lost password is precisely why you re-invite.
- **The default grant is `http:app`** — the node's control panel and nothing else. Not `http:*`
  (no dashboard is exposed until it is named), and never `tcp:`, which would splice a raw socket
  past the HTTP password layer entirely.

## 9. Relay and offline

Measured, not assumed (2026-07-31, two consumer NATs in Moscow, both on the public n0 relays):
pairing and every request worked with no inbound port on either side, but the round trip settled
at **0.3–0.4 s and never improved** — no direct path formed between the two NATs, so every request
crossed a relay in Frankfurt or Virginia. That is the number a **self-hosted relay** buys back:
functionally nothing changes, but a relay near the users turns a transatlantic round trip into
tens of milliseconds. Worth knowing before promising interactive latency over the default relays.

`RelayMode` is per node. Default is n0's public relays plus DNS/pkarr discovery
(`presets::N0`). A self-hosted relay (`iroh-relay`'s `server` feature, `RelayMode::Custom`) is
supported for independence. On a shared LAN, `RelayMode::Disabled` plus mDNS address lookup
gives direct QUIC with no internet at all. Off-LAN with no outbound connectivity is impossible
by construction — the minimum is one outbound UDP/443 session.

---

## Checklist

Each item ships with unit tests in the same file.

### A — front door (`adi-hive`)

- [x] A1 Expose the crate as a library (`lib.rs`) beside the binary, so the gateway can reuse
      the config loader and route table without duplicating it.
- [x] A2 Implement `proxy.path`: a service may claim a path prefix on another service's host.
      Longest-prefix wins; host-only routes keep working unchanged.
- [x] A3 Route every `*.n.adi` host to the local gateway port; parse `(service, node)`.
- [x] A4 A "mesh gateway unavailable" error page, distinct from 404 and 502, for a `*.n.adi`
      host when no gateway is configured or it cannot be reached. The *not-paired* page belongs
      to the gateway (C), not here: the front door must not know about the fleet registry — it
      sees a reserved suffix and hands the connection on. That boundary is what keeps "routes
      bytes" and "knows about peers" separate.
- [x] A5 TLS: cover `*.<node>.n.adi` in the leaf's SAN list.

### B — fleet registry (`adi-mesh`)

- [x] B1 Name validation (one DNS label, lowercase, length).
- [x] B2 The registry: petname → {key, nickname, paired_at, grants, auth}. Load/save.
- [x] B3 Pairing: accept a nickname, resolve collisions by suggestion, never fail on a clash.
- [x] B4 Pinning: a changed nickname is reported, never applied silently.
- [x] B5 Local rename, and lookup by petname and by key.
- [x] B6 Grants: default-deny, `http:<service>` / `http:*` / `tcp:<addr>:<port>` / `ctl:*`.

### C — gateway (`adi-mesh`)

- [x] C1 The `adi/mesh/http/1` frame: encode/decode, version and length validation.
- [x] C2 Status byte encode/decode, with a reason string per status.
- [x] C3 Basic auth: parse the header, salted-SHA-256 verify, constant-time compare, 401 with
      `WWW-Authenticate`; applies to upgrade requests too.
- [x] C4 Host parsing: `<service>.<node>.n.adi` → `(service, node)`, rejecting anything else.
- [x] C5 Connection pool: one iroh connection per peer, a bi-stream per request, reconnect with
      backoff.
- [x] C6 The node side: resolve the service label against the node's hive route table, honour
      grants, splice.

### D — dashboards

- [x] D1 Scaffold emits `proxy.path: /api` for the backend on the frontend's host.
- [x] D2 `frontend/index.ts` stops reading `ports/registry.json` and stops injecting
      `backendPort`; `ctx.api.base` becomes the relative `/api`.
- [x] D3 Existing dashboards migrate on next read.

### F — wiring (found while building A–D)

- [x] F1 `adi-core/src/dns.rs` generates the front-door `hive.yaml` and does not emit
      `proxy.mesh_gateway`. Until it does, the `*.n.adi` route exists in code but is never live
      on a real machine.
- [x] F2 Pairing must append the new petname to the front door's `proxy.mesh_nodes`, so the next
      hive start mints a leaf covering `*.<node>.n.adi`. Routing works immediately; only HTTPS
      waits for that restart.
- [x] F3 The panel links a dashboard as `http://127.0.0.1:<frontend_port>`
      (`adi-webapp/src/pages/dashboards.rs`). That URL now yields a page whose `/api` does not
      route. Link the dashboard's host instead — needs a `host` field on the `Dashboard` DTO in
      `adi-webapp-api/src/types.rs`.

### E — node (later rounds)

- [x] E1 systemd backend in `adi-core`'s supervisor abstraction.
- [x] E2 `apps/linux/build.sh` — static musl package with the five binaries.
- [x] E3 Pull-only bootstrap: one-time pairing token, node dials out, no inbound ssh needed.
- [x] E4 Fleet page in the control panel: nodes, status, grants, audit.
