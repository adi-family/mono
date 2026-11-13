# Webhook relay — pull-based inbound webhooks

A sender's webhook (Stripe, GitHub, a SaaS integration) cannot reach a node today:
`/api/hooks/<name>` only answers on the loopback bind adi-app itself listens on
(`listen_addr`, `adi-app/src/main.rs:384`, bound at `:213-215`), and nothing in `adi-app`'s own
request dispatch checks a credential — the Basic-auth gate that `docs/fleet.md` §5 describes lives
in the *mesh gateway* (`adi-mesh/src/gateway.rs`), on the path a peer's request takes through the
front door, not in `adi-app`'s dispatch itself. A webhook sender is neither on the loopback
interface nor a paired mesh peer, so there is no path in for it at all. This is task ADI-MONO-220.
The design is decided — read the task before this document; what follows turns it into a wire
contract, grounded in the code as it exists at `main` `9d19100` (2026-11-13).

**Status: design, not built.** Igor reviews this before any code is written (the task's phase 1).
Wherever the code contradicts the design, or the design leaves something undecided, the point is
listed under §14 "Open questions" and a recommendation is given — not decided silently. One
contradiction is serious enough to call out here, up front, rather than burying it in §14: **the
webhook trigger execution path has no way to see the sender's HTTP headers at all, today.**
`adi_triggers::fire::launch` (`crates/adi-triggers/src/fire.rs:77-132`) builds a code block's
environment from the trigger's `extra`, its name, its kind, and the request *body* — there is no
headers parameter anywhere on this path, and `handlers::hook_trigger`
(`adi-webapp-api/src/handlers/triggers.rs:205`) is called with `(store, name, query, payload)`,
never the request's headers. The `github-push` preset (`adi-triggers/src/presets.rs:230-256`)
parses the payload JSON directly and never checks `X-Hub-Signature-256`; no preset in the tree
checks a provider HMAC signature at all. So the design's whole safety argument for the public and
enterprise modes — "the relay holds no sender secrets; protection is the unguessable endpoint id
plus HMAC verification of the sender's signature **on the node**" — currently has no node-side
leg to stand on. §14 Q1 covers this; it should be fixed before or alongside phase 2, independent of
the relay's own existence, because it is already true of direct `/api/hooks/<name>` calls.

## 1. Why a separate service, and not `apps/channel-router`

`apps/channel-router` is the closest existing precedent for "a node makes an outbound connection
to a cloud service so something can reach it" — and the task explicitly asks for a different
service, not an extension of it. Both reasons are concrete, not just taste:

- **It is a Cloudflare Worker, and that is structural, not incidental.** `wrangler.toml:1-4`
  says why: the held-open per-node WebSocket is a Durable Object, because "Pages Functions cannot
  hold one". `[[durable_objects.bindings]]` and `[[d1_databases]]` (`wrangler.toml:38-60`) are
  Cloudflare-account resources — there is no Docker image of this Worker a customer could run on
  their own AWS box, because a Worker isn't a binary that runs anywhere else. The relay's hard
  requirement — "one small self-hostable binary + Docker image, local SQLite" — is unbuildable on
  this foundation.
- **The trust model and the delivery shape are both inverted.** A node's channel connection is
  registered once with a bearer token the router mints at `POST /register`
  (`adi-channels/src/connect.rs:64-71`, `RouterApi::register`) and then rides a single long-lived
  authenticated WebSocket (`adi-channels/src/client.rs`, `RouterClient::run`) for the life of the
  connection — the token authenticates the *session*, once, and every push after that rides inside
  it. The relay has no session to authenticate once: `register`, `pull` and `delete` are
  independent, infrequent, signed HTTP calls with no persistent connection between them (a
  long-poll is not a socket), so each one has to carry its own proof on its own — which is exactly
  what asymmetric per-request signing is for (§3) and a bearer token is not, especially once
  self-host means an operator who runs no backend to mint one against. A relay is also one
  Rust/Go/whatever binary and a SQLite file; a channel-router instance's state is a Durable Object
  and a D1 table. Folding one into the other would make the self-hostable half of the design a
  mode of a Cloudflare Worker, which it structurally cannot be.
- **`hooks.withadi.dev` is already taken**, by channel-router's own inbound webhook from Telegram
  and Slack (`wrangler.toml:30`, live since ADI-MONO-124). The relay's proposed `webhooks.withadi.dev`
  is one letter away and does a different job for a different audience — see §14 Q2.

## 2. Identity: the node's keypair

Every node→relay request is signed, never bearer-authenticated, because `register`/`pull`/`delete`
have no session to lean on (§1) and self-host has no backend to issue a token from. The node
**generates two separate keys**, not one:

- **ed25519 signing key** — proves which node is making the request (§3).
- **X25519 sealing key** — the relay seals a payload to this key on arrival (§4); the node is the
  only thing that can open it.

**Two keys, not a derivation of one, decided for the same reason `adi-mesh`'s own device-key file
gives for not reusing the mesh identity key to wrap secrets
(`adi-mesh/src/devicekey.rs:1-18`): "the mesh identity key authenticates a *connection*; reusing it
here would make 'can pair as you' and 'can read your subscription' one capability."** The same
argument holds between signing and sealing: "can prove this request came from the node" and "can
read this node's inbox" are different capabilities, and an Ed25519-to-X25519 birational conversion
(the kind `crypto_sign_ed25519_pk_to_curve25519` performs) is exactly the "clever" shortcut that
family of bugs comes from — libsodium ships it as an explicit, separate function rather than an
implicit one for that reason. Two independent 32-byte keys cost nothing extra to generate or store.

**Stored as raw files, not through `adi-secrets`.** `adi-secrets::Secrets` (`crates/adi-secrets`)
is for env-var-shaped values a *run* resolves into its environment (`Secrets::resolve`,
`lib.rs:610`) — AEAD-sealed under a master key any agent run on the box can decrypt
(`docs/byo-credentials.md` §8: "readable by any run on the box, whenever it liked"). The relay
signing/sealing keys are node identity material, exactly the shape `adi-mesh/src/identity.rs`
(the mesh `EndpointId` key) and `adi-mesh/src/devicekey.rs` (the BYO-credentials wrap key) already
are, and both of those are a raw 32-byte file under a module directory, `0600`
(`adi-config/src/fsutil.rs:94`), via `Config::module(..).write_raw`/`read_raw`
(`adi-config/src/module.rs:83-102`) — never through `adi-secrets`. The task's "private key stays
in node secrets" is read here as "stays on the node, never leaves it", not literally "goes through
the `adi-secrets` crate" — putting it there would make it resolvable into a run's environment,
which it must never be. Recommended home: a new `adi-relay` crate (§9), its own module dir
(`~/.adi/mono/relay/`, not `mesh/` — this key pair has nothing to do with mesh/fleet identity, it
is a wholly separate node↔cloud-service relationship), two files: `signing.key` (ed25519 seed, 32
bytes), `sealing.key` (X25519 secret, 32 bytes), generated lazily on first use with exactly the
`load_or_create`/`load_or_create_in` shape `identity.rs:19-44` and `devicekey.rs:40-64` already use
— same "wrong length is rejected, not rotated" test, same `0600` test.

**OpenSSH format, for the public half only.** The ed25519 *public* key is rendered as
`ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI…` (RFC 4716-ish base64 of the standard OpenSSH wire
format) — the format the task names, and the one a self-host operator already knows how to paste
into a file. No crate in the workspace does this today (`rg ed25519|ssh-key` in `Cargo.toml` finds
nothing — iroh pulls in `ed25519-dalek` transitively, but the workspace doesn't depend on it
directly). Recommended new dependencies: `ed25519-dalek` (sign/verify) and RustCrypto's `ssh-key`
(parses and renders the OpenSSH line, and parses an `authorized_keys` file for self-host, §7).

**Key rotation: not built in v1, by design, and that is a real limitation.** A relay has no
account to rotate a key *within* — the public key **is** the identity. Rotating means generating a
fresh pair and registering fresh endpoints under it; nothing migrates the old key's endpoints to
the new one. That matches how this codebase already treats an analogous case: re-pairing a mesh
peer is a fresh pairing, not an in-place key swap. The cost is real: every sender has to be given
a new URL. §14 Q3 flags whether that friction is acceptable or worth a "transfer my endpoints to a
new key" verb (signed by both the old and the new key) later.

## 3. Request signing

Every node→relay call — `POST /v1/endpoints`, `GET /v1/endpoints`, `DELETE /v1/endpoints/<id>`,
`GET /v1/pull` — carries four headers and no body-level secret:

```
X-Relay-Key:       ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI...      (the full OpenSSH public key line)
X-Relay-Timestamp: 1793000000                                     (unix seconds)
X-Relay-Nonce:     <16 random bytes, base64url, no padding>
X-Relay-Signature: <base64 ed25519 signature over the canonical string below>
```

**The full public key rides on every request, not a fingerprint.** There is no prior session to
look a fingerprint up against on the *first* call (`register` is itself signed, and is the first
thing the relay ever hears from a given key) — so the signature has to be self-contained and
verifiable with nothing but the request itself. The relay verifies the signature against the key
in the header, then separately decides whether that key is *authorized* (self-register in public
mode, licence-check in enterprise, `authorized_keys` lookup in self-host — §6) and, for `pull` and
`delete`, that the key's fingerprint matches the endpoint's recorded owner.

**Canonical string**, newline-joined, no body for a bodyless `GET`:

```
METHOD "\n" PATH_AND_QUERY "\n" TIMESTAMP "\n" NONCE "\n" SHA256_HEX(BODY)
```

`PATH_AND_QUERY` is the exact request-target as sent (`/v1/pull?batch=50`), so a proxy that
rewrites the query string invalidates the signature rather than silently changing what was asked
for. `SHA256_HEX(BODY)` is the hash of the empty string for a request with no body.

- **Clock-skew window: ±300 seconds.** A `Timestamp` outside that window is `401
  stale_timestamp` before the signature is even checked (cheaper to reject). Five minutes is
  generous enough for ordinary clock drift and network latency on a request that is not a
  long-lived credential (unlike a mesh pairing invite, which lives 10–15 minutes precisely because
  it has to survive being carried to a different machine, `docs/fleet.md` §8) and short enough that
  a captured request is useless well before anyone could do anything with it.
- **Nonce replay store, per key, bounded by the same window.** `nonces(owner_fingerprint, nonce,
  expires_at)`, primary key `(owner_fingerprint, nonce)` — a nonce seen twice inside the skew
  window is `401 replayed_nonce`; a background sweep (§9) prunes rows whose `expires_at` has
  passed, so the table never holds more than ~300 seconds of traffic per key.
- **`pull` is scoped to the key, not to one endpoint.** There is no endpoint id in `GET /v1/pull`'s
  path or query — it answers with whatever is queued across *every* endpoint this key owns on this
  relay, which is what "one poll per relay, not per hook" (the task's own words) requires.

## 4. Encryption at rest

**What's sealed: the whole record, headers included.** One JSON object —
`{"method","headers","query","body_b64"}` — sealed as a single blob, so the relay's own database
never holds an unsealed header either (a Stripe signature header is exactly the kind of thing that
must not sit in plaintext next to the payload it signs). What stays in the clear, because the
relay needs it to operate without opening anything: `endpoint_id`, `received_at`, `expires_at`,
and the plaintext byte length of the sealed JSON — needed to enforce the mode's body-size limit
and a per-endpoint pending-bytes cap without ever decrypting (§9).

**Envelope, modelled directly on `adi-mesh/src/lease_seal.rs`** (the BYO-credentials unlock seal —
the one other place in this codebase anonymously encrypts a value to a known X25519 public key over
an untrusted hop):

```
ephemeral X25519 keypair, fresh per record, discarded after sealing     (never reused — lease_seal.rs:19-21's reasoning)
ECDH(ephemeral secret, node's static sealing public key)
HKDF-SHA256(ikm = ecdh output, info = "adi-webhook-relay-seal-v1")  →  32-byte key
XChaCha20-Poly1305, random 24-byte nonce, AAD = "<endpoint_id>|<record_id>"
stored: ephemeral_pubkey, nonce, ciphertext
```

The AAD binds a ciphertext to the one record it was sealed for, the same reasoning
`lease_seal.rs:22-28` gives for its own AAD: a row copied or replayed into a different record's
slot fails to decrypt instead of opening as the wrong thing. Unlike `lease_seal`'s session key,
there's no server-side ephemeral half or epoch here — the recipient is the node's **long-lived**
sealing public key, submitted once at `register` (§6) and good until the key is rotated (§2).

**The node needs its own `open_value`-shaped counterpart**: ECDH against the stored
`ephemeral_pubkey` with its own sealing secret, same HKDF info string, then open. This is new code
(nothing in the tree opens a sealed box against a long-lived recipient key today — `lease_seal`'s
opener runs device-side against an *ephemeral* session key), but it's the same four primitives
already in the workspace (`x25519-dalek`, `hkdf`, `sha2`, `chacha20poly1305` —
`Cargo.toml:113,210-215`), used the other way round.

**Honest limit, stated plainly because the task requires it:** plaintext exists in the relay's own
process memory from the moment Caddy's TLS termination hands it the request body until the seal
above completes — on the same host, never serialized anywhere unsealed, and never logged (the
design's "relay never logs bodies" is enforced by simply never writing a body to a log sink in the
relay binary, not by redaction after the fact). A compromise of that one host's root, or of the
relay or Caddy binary itself, can read a payload **at the instant it arrives**, indistinguishably
from any other reverse proxy anywhere. What encryption at rest buys is specifically protection
against a **later** compromise — a stolen disk, a leaked backup, a database dump — not against a
live attacker on the box at the moment of delivery. EBS encryption (default-enabled on the
account) is the second, orthogonal layer for exactly that later case: it protects a lifted
volume/snapshot, and does nothing for the live-memory case above.

## 5. Wire contract

JSON throughout, snake_case fields (matching every existing DTO in `adi-webapp-api/src/types.rs`,
e.g. `TriggerDto`/`HookAck` at `:3731`/`:3901` — no `rename_all(camelCase)` anywhere in this
codebase's wire types). Error body, every non-2xx response:

```json
{ "error": "bad_signature", "message": "human-readable detail" }
```

Codes used below: `bad_signature`, `stale_timestamp`, `replayed_nonce`, `unknown_key` (self-host:
not in `authorized_keys`), `licence_lapsed` (register only — see §6, a `pull`/registered endpoint
that lapses is *paused*, not an error), `rate_limited`, `payload_too_large`, `too_many_endpoints`,
`not_found`.

### 5.1 `POST|GET|PUT|PATCH|DELETE /h/<endpoint-id>` — sender ingress, unauthenticated

The one endpoint with no signature, because the sender can't produce one (§6's "no shared secrets
on the wire" is about the node↔relay leg; a sender is never asked to hold a relay credential). Any
method is accepted and stored verbatim — the relay doesn't interpret the request, it captures it,
the same stance `/api/hooks/<name>` already takes by accepting both `GET` and `POST`
(`adi-app/src/main.rs:1118`, "GET also supports provider pings").

- Answers **`202 { "ok": true }`** immediately, before anything but the cheap checks below —
  there is no trigger to run synchronously here, unlike `/api/hooks/<name>`'s `ok_json(&HookAck)`,
  which only returns once `store.fire` has spawned something.
- `404 not_found` — unknown, deleted, or never-registered endpoint id. Unlike
  `hook_trigger`'s deliberate "unknown and disabled read identically" (so an attacker can't probe
  internal trigger names, `handlers/triggers.rs:208`), there is nothing to protect by hiding this
  here: an endpoint id is already ~128 bits of relay-minted randomness (§6), so a 404 tells a
  legitimate integrator they mistyped the URL and tells an attacker nothing they didn't already
  fail to guess.
- `413 payload_too_large` — over the mode's `max_body_bytes` (§7).
- `429 rate_limited` — over the endpoint's `max_requests_per_minute`.
- **A paused endpoint (§6) still answers `202` and still queues.** Refusing intake the moment a
  licence lapses would make Stripe's own retry/alerting treat a billing hiccup as an integration
  failure, and (§9) a paused endpoint's records still expire on the same retention clock as
  everyone else's — nothing is kept longer by being paused, only *withheld* from `pull` (§5.4)
  until the daily recheck clears it.

### 5.2 `POST /v1/endpoints` — register

```json
// → 
{ "sealing_key": "<base64 X25519 public key>", "retention_secs": 300, "licence_code": "…" }
```

`retention_secs` optional (defaults per mode, §7); `licence_code` required and checked only in
enterprise mode (§6). Signed per §3; the key in `X-Relay-Key` is recorded as the endpoint's owner.

```json
// ← 201
{ "endpoint_id": "wh_7f3kq9m2m2x8qr4n", "retention_secs": 300, "url": "https://eu.webhooks.withadi.dev/h/wh_7f3kq9m2m2x8qr4n" }
```

**The relay mints `endpoint_id`, not the node.** This mirrors `apps/channel-router`'s own
`/register`, which mints the connection id server-side and hands it back
(`adi-channels/src/connect.rs:23-26,73`, `registered.connection`) — the same reasoning applies here:
uniqueness across every node that has ever registered with this relay instance is the relay's own
namespace to guarantee, not something a node can promise about an id it picked itself. Format:
`wh_` + 22 random lowercase-base32 characters (~110 bits) — unguessable is the entire protection
a sender gets (§1), so this needs real entropy, not just a short random slug.

- `401 bad_signature` / `stale_timestamp` / `replayed_nonce` — signature checks, as everywhere.
- `402 licence_lapsed` — enterprise only, `licence_code` doesn't verify (§6).
- `403 unknown_key` — self-host only, the key isn't in `authorized_keys` (§6).
- `429 too_many_endpoints` — over the mode's `max_endpoints_per_key`, or (public only) over
  `max_registrations_per_ip_per_day`.

### 5.3 `GET /v1/endpoints` — list

Signed, no body. Lists every endpoint the caller's key owns:

```json
{ "endpoints": [
  { "endpoint_id": "wh_7f3kq9m2m2x8qr4n", "retention_secs": 300, "paused": false,
    "created_at": 1793000000, "pending": 3 }
] }
```

### 5.4 `DELETE /v1/endpoints/<id>` — delete

Signed. `403` if the caller's key fingerprint doesn't match the endpoint's recorded owner (not
`404` — telling an attacker who guessed a real id "that's not yours" versus "doesn't exist" is a
distinction worth keeping honest for the legitimate owner's own debugging, and the id itself isn't
the secret here, the key is). Deletes the endpoint row and every one of its queued records
immediately — not at their natural expiry. `204` on success, idempotent `404` on an id that never
existed or was already deleted.

### 5.5 `GET /v1/pull?batch=<n>` — long-poll, the node's only read path

Signed, no body. `batch` optional, clamped to the mode's `pull_batch_max` (§7; default omitted
means the max). Waits up to **25 seconds** for at least one record across every endpoint the
caller's key owns, returning immediately if one is already queued. 25s is chosen to sit safely
under common reverse-proxy/load-balancer idle timeouts (often 30s) with margin to spare; whoever
stands up Caddy in front of this (§10) must set its own read/idle timeout comfortably above it —
flagged for verification in phase 3, not asserted as already true of any particular Caddy default.

```json
{ "records": [
  { "endpoint_id": "wh_7f3kq9m2m2x8qr4n", "received_at": 1793000012,
    "sealed": { "ephemeral_pubkey": "…", "nonce": "…", "ciphertext": "…" } }
], "has_more": false }
```

**Delete-at-handout, no ack, no lease.** Every record included in a `200` response is deleted from
the relay's storage in the same transaction that selected it — `DELETE … RETURNING *` (SQLite
≥3.35, available through `rusqlite`'s bundled engine) does both atomically. The record existed on
the wire exactly once; there is no second copy anywhere to retry from. **Accepted loss case,
exactly as the task states it:** a node that crashes after the `200` lands but before it finishes
replaying every record in the batch (§8) loses whatever it hadn't replayed yet, permanently. This
is a deliberate simplicity trade — no ack protocol, no lease table on the relay, no redelivery
logic — and §14 Q4 is the one way this loss window is wider than that single sentence suggests.
A `200` with an empty `"records": []` means the 25s elapsed with nothing to return; a node's poll
loop treats that identically to "go around again", never as an error.

### 5.6 `GET /healthz` — unauthenticated

`200 OK`, plain text, for Docker's own `HEALTHCHECK` and whatever uptime monitor watches the box.
No signature, no mode-specific behaviour — it answers before any of the above logic runs.

## 6. The three modes

One binary, one `relay.toml`, `mode` picks the behaviour:

```toml
mode     = "public"                      # public | enterprise | self-host
bind     = "0.0.0.0:8080"                # Caddy fronts this; the relay itself never terminates TLS
db_path  = "/data/relay.db"
hostname = "eu.webhooks.withadi.dev"     # used to build the "url" field in a register response

[limits]
max_endpoints_per_key               = 5
max_body_bytes                      = 262144     # 256 KiB
max_requests_per_minute_per_endpoint = 60
max_registrations_per_ip_per_day    = 10
default_retention_secs              = 300
max_retention_secs                  = 900        # 15 min
pull_batch_max                      = 50
pull_long_poll_secs                 = 25

[licence]                                # enterprise only
service_url            = "https://licence.withadi.dev"
recheck_interval_secs   = 86400

[self_host]                              # self-host only
authorized_keys_path = "/etc/webhook-relay/authorized_keys"
shared_secret_env    = "RELAY_SHARED_SECRET"   # name of an env var; empty = not required
ip_allowlist          = []                      # CIDRs; empty = unrestricted
```

Defaults proposed below, within the ranges the task gives — Igor's to adjust:

| | public | enterprise | self-host (ships as) |
|---|---|---|---|
| `max_endpoints_per_key` | 5 | 100 | unlimited (`authorized_keys` is already the gate) |
| `max_body_bytes` | 256 KiB | 2 MiB (cap 5 MiB) | 2 MiB, operator sets |
| `default_retention_secs` / `max_retention_secs` | 300 / 900 | 3600 / 86400 | 300 / unlimited (operator sets) |
| `max_requests_per_minute_per_endpoint` | 60 | 600 | 600, operator sets |
| `max_registrations_per_ip_per_day` | 10 | n/a (licence-gated instead) | n/a |
| `pull_batch_max` | 50 | 200 | 200, operator sets |

- **Public — any key self-registers.** The first `POST /v1/endpoints` a relay instance sees from a
  key is accepted outright, subject only to the per-IP registration cap and per-key endpoint cap.
  No admin step, matching channel-router's own "open self-registration" since ADI-MONO-125.
- **Enterprise — key plus a valid licence.** `register`'s body carries `licence_code`; the relay
  calls out to the licence service once, synchronously, before answering `201`, and caches
  `(owner_fingerprint) → valid, checked_at` in `licence_status`. A background sweep re-verifies
  every row once every `recheck_interval_secs` (24h default). **A lapsed licence pauses every
  endpoint that fingerprint owns** (`endpoints.paused = true`) — `pull` filters paused endpoints out
  of its result set entirely (§5.5's response simply never contains them) while ingress (§5.1)
  keeps accepting. There is no endpoint to force an early recheck in v1 — a renewal takes up to 24h
  to take effect, which is §14 Q5.
- **Self-host — `authorized_keys`, operator-managed.** One `ssh-ed25519 AAAA… [comment]` per line,
  the exact OpenSSH `authorized_keys` format an operator already knows. **Reloaded by mtime, at
  most every 5 seconds** — the identical cadence `adi-mesh`'s gateway already uses for `fleet.toml`
  (`docs/fleet.md` §6, `RELOAD_INTERVAL`), reused here for the same reason: adding a node's key
  shouldn't need a relay restart, and polling an mtime is cheap enough to do on every request
  without a file-watcher dependency. A key not in the file is `403 unknown_key` at `register` (and
  everywhere else, since nothing gets this far without first registering). `shared_secret_env` and
  `ip_allowlist` are **defence in depth on top of the signature, not instead of it** — an optional
  extra header (`X-Relay-Shared-Secret`, constant-time compared, never part of the signed canonical
  string, since it's a static value a signature can't meaningfully protect any further) and a CIDR
  check against the connecting IP, both opt-in and both off by default.

## 7. Retention and purge

`retention_secs` is set once, at `register` (§5.2), clamped to `[0, max_retention_secs]` for the
mode. A record's `expires_at = received_at + endpoint.retention_secs`. A background sweep, every
30 seconds, in one query: `DELETE FROM records WHERE expires_at < strftime('%s','now')` — unclaimed
means exactly this: never reached by a `pull` before its clock ran out. The same sweep prunes
`nonces` past their skew window (§3) and `registrations_by_ip` rows older than 2 days (§6's public
cap only needs a rolling day). Independent of time, a **per-endpoint pending-record cap**
(proposed: 10,000) bounds storage for an endpoint nobody is polling at all — hit it and new
deliveries answer `429 rate_limited` rather than growing the database without limit while a
retention clock that hasn't expired yet keeps everything alive.

## 8. Relay implementation shape

**A new crate, `crates/adi-relay-server`** (the service) **and `crates/adi-relay`** (the client
half — signing, sealing, key management — used by `adi-app` and `adi-cli`, §12). Rust, for three
reasons that all matter together: it's the one language every other service binary in this
workspace (`adi-llm-gateway`, `adi-app`) already ships as, the crypto primitives this spec needs
(`x25519-dalek`, `hkdf`, `chacha20poly1305`) are already workspace dependencies proven in
`adi-mesh/src/lease_seal.rs`, and `rusqlite` with the `bundled` feature (already a workspace
dependency, `Cargo.toml:128`) compiles SQLite in rather than linking a system library — the same
property `adi-db`'s own doc gives for why it does this ("every target … ships the same engine and
version"), which is exactly what "one binary, one Docker image, runs the same on our box and a
customer's" needs.

**Deliberately *not* built on `adi-db` or `adi-config`.** Both tie a crate to
`~/.adi/mono/`'s store layout (`adi-db`'s own doc: "a pure library … over the `adi_config` store"),
which is the layout of a *node* in the adi-mono platform — a customer self-hosting the relay has
none of that installed, and shouldn't need to. The relay opens one `rusqlite::Connection` at the
path `relay.toml` names, with the same WAL pragmas `adi-db` documents for the same reason (readers
never block the writer), and nothing else about `adi-db` is reused.

Schema (§3, §4, §7 already gave the reasoning for each table):

```sql
CREATE TABLE endpoints (
  id                 TEXT PRIMARY KEY,         -- "wh_" + 22 base32 chars
  owner_fingerprint  TEXT NOT NULL,            -- sha256 of the OpenSSH public-key line
  owner_key          TEXT NOT NULL,            -- the full ssh-ed25519 line
  sealing_key        TEXT NOT NULL,            -- base64 X25519 public key, from register
  retention_secs     INTEGER NOT NULL,
  paused             INTEGER NOT NULL DEFAULT 0,
  created_at         INTEGER NOT NULL
);
CREATE INDEX idx_endpoints_owner ON endpoints(owner_fingerprint);

CREATE TABLE records (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  endpoint_id       TEXT NOT NULL REFERENCES endpoints(id),
  received_at       INTEGER NOT NULL,
  expires_at        INTEGER NOT NULL,
  byte_len          INTEGER NOT NULL,
  ephemeral_pubkey  BLOB NOT NULL,
  nonce             BLOB NOT NULL,
  ciphertext        BLOB NOT NULL
);
CREATE INDEX idx_records_endpoint ON records(endpoint_id);
CREATE INDEX idx_records_expires  ON records(expires_at);

CREATE TABLE nonces (
  owner_fingerprint TEXT NOT NULL,
  nonce             TEXT NOT NULL,
  expires_at        INTEGER NOT NULL,
  PRIMARY KEY (owner_fingerprint, nonce)
);

CREATE TABLE licence_status (                  -- enterprise mode only
  owner_fingerprint TEXT PRIMARY KEY,
  valid             INTEGER NOT NULL,
  checked_at        INTEGER NOT NULL
);

CREATE TABLE registrations_by_ip (             -- public mode only
  ip    TEXT NOT NULL,
  day   TEXT NOT NULL,                          -- "2026-11-13"
  count INTEGER NOT NULL,
  PRIMARY KEY (ip, day)
);
```

Abuse limits beyond §7's tables: `max_body_bytes` and `max_requests_per_minute_per_endpoint`
checked at ingress (§5.1) before the body is even fully read where the transport allows it;
`max_endpoints_per_key` and `max_registrations_per_ip_per_day` checked at `register` (§5.2); the
per-endpoint pending-record cap (§7) checked at ingress too, against a `COUNT(*)` on `records`.

## 9. Hosting (ours)

AWS, `eu-central-1` and `us-east-1`, one machine per region, no cross-region anything — each
region's SQLite file is local to that one machine's volume, and nothing in this design ever
replicates, forwards, or even compares data between regions, so "no cross-region replication" is
true by the absence of any code path that would do it, not by a flag that disables one. Two Docker
containers per machine (public, enterprise), each its own `relay.toml`, its own `db_path`, its own
hostname. Caddy in front, terminating TLS and reverse-proxying by hostname to the right container's
bound port — the relay binary itself never holds a certificate.

**Hostnames, as the task proposes:** `eu.webhooks.withadi.dev`, `us.webhooks.withadi.dev`,
`eu.ent.webhooks.withadi.dev`, `us.ent.webhooks.withadi.dev`. §14 Q2 flags the one thing worth a
second look before these go live: `hooks.withadi.dev` (channel-router, §1) and
`*.webhooks.withadi.dev` (this) are easy to mix up in support conversations despite being
unrelated services; not a reason to rename, a reason to say so plainly wherever both are
documented together.

**Adding a region is one script**, the same shape `scripts/deploy-relay-aws.sh` already uses for
iroh relays (`docs/fleet.md` §9): reserve an Elastic IP, open the security group for tcp 80/443 (no
UDP needed here, unlike the iroh relay's 7842), boot the box, install the Docker image and a
`relay.toml` per container, start Caddy. The same caveat that script's own doc states for iroh
applies unchanged: **the DNS A record must exist before Caddy's first start**, because its ACME
challenge is answered on `:80` by Caddy itself — standing up the box before the DNS record is
pointed at it is the one ordering mistake that silently wastes a certificate request.

## 10. Node side in `adi-mono`

**The trigger gains settings in `extra`, not new `TriggerManifest` fields.** `extra` is already
"named settings the code block reads … which keys matter is the preset's business — `secret` is
the one the platform itself reads" (`adi-triggers/src/trigger.rs:102-104`) — `secret` is exactly
this shape today: a platform-enforced, webhook-only setting that lives in the same
`BTreeMap<String, String>` as everything a preset defines. Three new keys, same mechanism, no
schema migration, no new `ALTER TABLE`:

- `extra["relay"]` — `"public-eu" | "public-us" | "enterprise-eu" | "enterprise-us" |
  "custom:<url>"`, or absent (not relay-backed — the trigger only answers direct `/api/hooks/<name>`
  calls, as today).
- `extra["relay_retention"]` — seconds, as a string, written by whatever UI/CLI flow does the
  registration.
- `extra["relay_endpoint"]` — the `endpoint_id` the relay minted (§5.2), written back once
  registration succeeds. Its presence is what the poller (below) uses to know which relay record
  belongs to which local trigger.

Saving a trigger with `relay` set (or changing it) is the moment `adi-app` calls out: register
against the named relay (or the `custom:<url>` one) with the node's sealing key, and only write
`relay_endpoint`/persist the trigger once that call succeeds — a failed registration is a failed
save, not a trigger left half-configured. Disabling, deleting, or changing a trigger's `relay`
calls `DELETE /v1/endpoints/<id>` **best-effort** — the same stance `docs/fleet.md` §10 takes for a
grant a transfer asks for ("a link that refuses, not a transfer that failed"): retention and the
purge sweep (§7) bound the relay-side cost of a delete that never lands, so nothing is lost by not
retrying it.

**Keys: one pair per node, not per trigger.** Generated lazily by the new `adi-relay` crate (§8),
exactly the `load_or_create` shape `adi-mesh/src/identity.rs` and `devicekey.rs` already use,
stored at `~/.adi/mono/relay/{signing,sealing}.key` — a sibling module to `mesh/`, deliberately not
inside it (§2). Every relay-backed trigger on the node shares the one pair; the relay tells them
apart by `endpoint_id`, not by key.

**The poller is built into `adi-app`, never a user trigger.** One `tokio::spawn`'d loop per
*distinct relay in use* (collected from every enabled trigger's `extra["relay"]`), started from
`main()` beside every other background loop in that crate — `prober::start`, `event_log::start`,
and BYO-credentials' own `lease_worker.rs`, which `docs/byo-credentials.md` §9 describes as "this
plus the timer", the same pattern this needs. Each loop: sign and send `GET /v1/pull` (§5.5),
reconnect with exponential backoff on any transport failure (1s, capped at 30s, reset to 1s on the
next success — no existing precedent in this tree pins exact numbers for this shape, so these are
proposed, not measured), and on a non-empty batch, for each record: open its seal (§4) with the
node's sealing secret, find the trigger(s) whose `extra["relay_endpoint"]` matches the record's
`endpoint_id`, and **replay it through a real HTTP call to the node's own loopback listener**,
`POST /api/hooks/<name>?secret=<extra["secret"]>` — not an in-process call straight into
`handlers::hook_trigger`. This is deliberate: it is the one way a relay-delivered hook and a
directly-received one are guaranteed to pass through **exactly the same** gating
(`enabled`, `secret`, `trigger_on` — `handlers/triggers.rs:205-245`) with no second copy of that
logic to keep in step as it changes. The unsealed headers travel with the replay once §14 Q1's
prerequisite lands (there is nowhere to put them yet otherwise). HMAC verification of a sender's
own signature (Stripe, GitHub) stays exactly where the design says it must — in the trigger's code
block — unaffected by any of this except that, after Q1, it finally has headers to check.

**Panel.** The trigger editor's webhook section gains: a relay picker (the four hosted options
plus "custom URL"), a retention field, and once registered, the **public URL** to copy into the
sender (`https://eu.webhooks.withadi.dev/h/wh_…`, exactly the `url` field §5.2 returns) and a
**copy public key** button for self-host operators who need to add the node to their
`authorized_keys`. No QR code here — unlike a mesh pairing invite (`docs/fleet.md` §8), this key
is not a bearer credential that benefits from a short-lived display; it's a public key, meant to
be pasted into a file and left there.

**CLI**, mirroring the existing `adi-mono triggers save --extra k=v` shape (`adi-cli/src/triggers.rs`)
rather than adding new flags: `--extra relay=enterprise-eu --extra relay_retention=3600` works with
zero CLI changes, since `extra` is already a repeatable `key=value` flag. The one new verb needed
is read-only: `adi-mono triggers relay-key` (or folded into `triggers show`), printing the node's
OpenSSH public key for pasting into a self-host `authorized_keys` file.

## 11. Threat model and honest limits

- **A relay operator (including us) can read a payload for the instant it takes to seal it** (§4)
  and can always deny service — drop a delivery, stall a `pull`, delete an endpoint. Nothing in
  this design protects against a relay that chooses not to cooperate; the design's only promise is
  that a *passive* compromise of the relay's storage (a stolen disk, a dumped database) yields
  nothing but ciphertext it cannot open, because it never held the sealing secret.
- **The relay can correlate, even sealed.** Endpoint ids, timing, sender IPs, and request sizes are
  all visible to the relay operator regardless of sealing — sealing hides *content*, not *traffic
  shape*. A relay seeing bursts of 200-byte POSTs to one endpoint every few seconds is a webhook,
  whoever's running it.
- **An unguessable endpoint id is the sender's entire defence**, and it is exactly as strong as its
  entropy (~110 bits, §5.2) and exactly as leaky as whatever the sender does with the URL — logging
  it, putting it in a GET query a browser's history remembers, screen-sharing a config page. The
  relay cannot protect against a leaked URL; only the node's own signature verification of the
  *sender's* payload (Q1) can tell a replayed capture from the real thing once the id is known.
- **A compromised node signing key can register new endpoints and read anything already queued
  for the node's existing ones (any endpoint `pull` returns records for, keyed by fingerprint)**
  until the key is rotated (§2) — which, in v1, means every legitimate endpoint also has to be
  re-registered under the new key, at the same cost an attacker's compromise imposes. There is no
  faster "revoke just this key" lever than that in this design.
- **Self-host's `authorized_keys` is only as good as the file it reads.** An operator who commits
  it to a public repo, or who never rotates a departed employee's key out of it, has the exact
  failure mode real SSH `authorized_keys` files have always had — this design inherits that
  tradeoff deliberately, for the familiarity, rather than inventing a different one.
- **Delete-at-handout's accepted loss is wider than "a node crash mid-replay".** See §14 Q4 — any
  failure during replay (the local trigger disabled between registration and delivery, a
  `trigger_on` project mismatch, the node's own `/api/hooks` momentarily unreachable) loses the
  record identically, because the relay has already deleted its only copy by the time any of those
  are discovered.

## 12. Open questions

Each with a recommendation, not a decision — Igor's to make.

**Q1 — Header passthrough is missing from the webhook trigger path entirely, today, independent of
the relay.** `fire::launch` has no headers parameter (`fire.rs:77-132`); no preset checks a
provider's HMAC signature (`presets.rs`). *Recommendation:* file this as its own fix, land it
before or alongside phase 2 — write the captured headers to an `ADI_HEADERS_FILE` (JSON) the same
way `ADI_PAYLOAD_FILE` already works, threaded through `hook_trigger` → `Triggers::fire` →
`fire::launch`, for both the direct `/api/hooks/<name>` path and the relay's replay (§10). Without
it, the design's stated security model for public/enterprise mode is aspirational.

**Q2 — Hostname proximity.** `webhooks.withadi.dev` beside the already-live `hooks.withadi.dev`
(channel-router, §1) invites confusion in support and docs. *Recommendation:* keep the proposed
names (changing a *second* service's hostname is more disruptive than documenting the distinction
once, prominently, in both services' docs) but don't let the two ever appear in the same sentence
without saying which is which.

**Q3 — No key-rotation verb.** (§2) *Recommendation:* ship v1 without one, as designed; revisit only
if self-host operators report the "repaste every URL" cost as a real problem, not pre-emptively.

**Q4 — Delete-at-handout's loss window is wider than "a crash between pull and trigger".**
(§5.5, §11) *Recommendation:* state the true scope plainly in whatever customer-facing self-host
guide phase 5 produces — "a record is gone the instant your node acknowledges the pull, whether or
not the trigger it was meant for actually ran" — rather than letting the task's narrower framing
stand uncorrected in a document customers will read.

**Q5 — A lapsed-then-renewed enterprise licence takes up to 24h to unpause**, because the only
recheck is the daily sweep (§6). *Recommendation:* add a cheap forced-recheck path later (an admin
CLI verb against the relay, or simply restarting the container, which re-checks at startup) rather
than building a webhook-from-the-licence-service for this in v1 — a renewal is rare enough that a
24h worst case, stated honestly, is an acceptable launch gap.

**Q6 — `/v1/verify` does not exist anywhere in this codebase.** The only existing integration with
`licence.withadi.dev` is `is_licence_service`/`classify_error`
(`adi-webapp-api/src/handlers/update.rs:200-254`), which reads **update-manifest fetch status
codes** (401/402/403/404/409 against `GET …/v1/updates/manifest`) as licence states for the
*update channel* — a different feature, a different request shape, driven by `curl -fsSL` rather
than a programmatic client. There is no Rust HTTP client for the licence service in this tree at
all, and no evidence `/v1/verify` is a route that service already serves. *Recommendation:*
confirm with whoever owns `licence.withadi.dev` whether `/v1/verify` exists and what it expects
before phase 2 builds against it; if it doesn't exist yet, this spec's enterprise-mode registration
flow (§6) is blocked on that service gaining a route, not just on this relay being built.

## Checklist

Phase numbers match the task. This document is phase 1.

### Phase 1 — this spec
- [x] Written, grounded in the code as of `main` `9d19100`. Awaiting Igor's review.

### Before/alongside phase 2
- [x] Q1: thread request headers into the webhook trigger execution environment
      (`ADI_HEADERS_FILE`), for both `/api/hooks/<name>` directly and the relay replay path.
      Landed ADI-MONO-221 (`f42d85d`): `hook_trigger` takes the request's `HashMap<String, String>`
      headers, `Triggers::fire` gained a matching `headers` parameter (`fire_event` and the
      supervisor's own launch stay `None` — neither a platform event nor a background process has
      request headers), and `fire::launch` writes them as JSON to `ADI_HEADERS_FILE`, file-only
      (no inline `ADI_HEADERS`, unlike the payload's small-body exception — a header map's size
      isn't bounded by the same `max_body_bytes` reasoning). The relay replay path (§10) still has
      to pass its unsealed headers through once WR-C builds it; this only unblocks it.

### Phase 2 — relay service, image, tests
- [x] `crates/adi-relay` (client: signing, sealing, OpenSSH encode/decode, key load-or-create,
      ADI-MONO-222 "WR-A") and `crates/adi-relay-server` (the service binary, "WR-B"): schema
      (§8), the five routes (§5), signing verification + nonce replay (§3), sealing/unsealing
      (§4), the three modes (§6), retention sweep (§7). 82 tests across the two crates, `cargo
      clippy -p adi-relay -p adi-relay-server --all-targets` clean.
- [x] Docker image: `crates/adi-relay-server/Dockerfile` — multi-stage (`rust:1-slim-bookworm`
      builder, `debian:bookworm-slim` runtime), non-root user, `/data` volume, `HEALTHCHECK`
      against `/healthz`. **Written, not build-verified** — this box has no Docker; the deploy
      agent builds it.
- [x] Tests: signature/replay/skew rejection; seal-then-open round trip against a fixture key;
      delete-at-handout is atomic under concurrent pulls; retention purge; each mode's limits;
      public self-registration; enterprise licence-lapse pauses pull but not ingress; self-host
      `authorized_keys` reload and the optional shared-secret/IP-allowlist layers; the enterprise
      licence client against the real beam contract (§14 Q6), including a real in-process
      `/v1/verify` HTTP round trip.

### Phase 3 — deploy eu/us, public + enterprise, AWS + DNS
- [ ] Confirm `/v1/verify` (Q6) before wiring enterprise licence checks against it.
- [ ] `scripts/deploy-webhook-relay-aws.sh <region>`, mirroring `deploy-relay-aws.sh`'s shape
      (§9) — DNS record before first Caddy start, security group tcp 80/443 only.
- [ ] Verify the 25s `pull` long-poll sits under Caddy's own configured idle/read timeout (§5.5).
- [ ] Both regions, both modes, four hostnames live; confirm no cross-region data path exists by
      construction (§9).

### Phase 4 — node side in `adi-mono`
- [ ] `extra["relay"]` / `extra["relay_retention"]` / `extra["relay_endpoint"]` on webhook
      triggers (§10); register-on-save, best-effort delete-on-remove.
- [ ] Node keypair: `adi-relay`'s `load_or_create` for `signing.key`/`sealing.key` under
      `~/.adi/mono/relay/`.
- [ ] The built-in poller in `adi-app`: one loop per distinct relay in use, reconnect/backoff,
      unseal, replay via a real loopback HTTP call to `/api/hooks/<name>`.
- [ ] Panel: relay picker + retention field on the trigger editor, public URL display, copy
      public key.
- [ ] CLI: `adi-mono triggers relay-key` (or folded into `triggers show`).

### Phase 5 — self-host guide
- [ ] Docker Compose example, `relay.toml` for self-host mode, `authorized_keys` walkthrough,
      the optional shared-secret/IP-allowlist layers, and Q4's honest statement of the
      delete-at-handout loss window.
