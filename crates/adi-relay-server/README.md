# adi-relay-server — pull-based inbound webhooks

A sender's webhook (Stripe, GitHub, a SaaS integration) cannot reach a node directly:
`/api/hooks/<name>` only answers on the loopback bind `adi-app` itself listens on, and nothing in
`adi-app`'s own request dispatch checks a credential a stranger on the internet could hold. This
crate is the other half of the answer (ADI-MONO-220): a small, self-hostable relay a sender posts
to instead. The relay never connects to the node — the node reaches out, on a long-poll, the same
principle `apps/channel-router` uses for a different audience (see "Why not channel-router" below).

A sender `POST`s to `https://<relay>/h/<endpoint-id>`; the relay answers `202` at once and stores
`{method, headers, query, body}`, sealed so it can't read its own database back; the node
long-polls `GET /v1/pull`, gets everything queued across every endpoint it owns on that relay, and
replays each record into its own `/api/hooks/<name>` — so an existing webhook trigger's code is
unaffected either way. Records are **deleted the instant a `pull` returns them** — no ack, no
lease, no redelivery. See "Honest limits" below before relying on this for anything where that
loss is expensive.

This binary runs three ways, picked by `relay.toml`'s `mode`:

- **`public`** — we host it (`eu.webhooks.withadi.dev`, `us.webhooks.withadi.dev`). Any key
  self-registers; strict limits.
- **`enterprise`** — we host it too (`eu.ent.webhooks.withadi.dev`, `us.ent.webhooks.withadi.dev`).
  Registration needs a valid ADI licence, checked against `adi-licence-service`; generous limits.
- **`self-host`** — you run it. Only keys you list in an `authorized_keys` file are accepted; no
  licence check, no self-registration; limits you set.

The client half (keys, request signing, sealed-box open/seal) is `crates/adi-relay`, used by both
this binary and by `adi-app`/`adi-cli` on the node side.

## Why not `apps/channel-router`

`channel-router` is the closest existing precedent for "a node dials out so something can reach
it", and this is deliberately a different service, for two concrete reasons:

- **It's a Cloudflare Worker, structurally.** The held-open per-node WebSocket is a Durable
  Object; there is no Docker image of a Worker a customer could run on their own box. This
  relay's hard requirement — one small self-hostable binary, local SQLite — is unbuildable on
  that foundation.
- **The trust model is inverted.** A channel-router node authenticates one long-lived WebSocket
  session, once. This relay's `register`/`pull`/`delete` calls are independent, infrequent, and
  have no session to lean on — each one is signed on its own, which asymmetric per-request
  signing is for and a bearer token is not, especially once self-host means an operator who runs
  no backend to mint one against.

`hooks.withadi.dev` (channel-router's own inbound webhook, live since ADI-MONO-124) and
`*.webhooks.withadi.dev` (this relay) are one letter apart and serve different audiences — don't
let the two appear in the same sentence without saying which is which.

## Identity: the node's keypair

Every node→relay request is signed, never bearer-authenticated. A node generates **two**
independent 32-byte keys, not one, stored as raw files under `~/.adi/mono/relay/` (never through
`adi-secrets`, which resolves values into a run's environment — this is node identity material
that must never do that):

- **`signing.key`** (ed25519) — proves which node is making a request.
- **`sealing.key`** (X25519) — the relay seals a payload to this key on arrival; only the node can
  open it.

Two keys rather than deriving one from the other for the same reason `adi-mesh`'s own device-key
file gives for not reusing the mesh identity key to wrap secrets: "can prove this request came
from the node" and "can read this node's inbox" are different capabilities, and collapsing them
into one key is exactly the kind of shortcut that family of bugs comes from.

The public signing key renders as a one-line OpenSSH public key, `ssh-ed25519 AAAA…` — the format
a self-host operator already knows from `~/.ssh/authorized_keys`. `adi-mono triggers relay-key`
prints it.

Naming a relay on a webhook trigger's `relay` setting — `adi-mono triggers save <name> --kind
webhook --extra relay=public-eu` (`public-eu` | `public-us` | `enterprise-eu` | `enterprise-us` |
`custom:<url>`), or the panel's own trigger editor — registers `POST /v1/endpoints` for it before
the save lands: a failed registration fails the save with the relay's own reason, never leaving a
trigger half-configured with a `relay` nobody answered for. Both callers share one
implementation, `adi_relay::register` (ADI-MONO-227); the CLI prints the resulting `/h/<id>` URL.
Changing or clearing `relay` on a later save best-effort deletes the endpoint it replaced.

**No key rotation in v1.** The public key *is* the identity; there is no account to rotate a key
within. Rotating means a fresh pair and fresh endpoints — every sender has to be given a new URL.

## Request signing

Every node→relay call (`POST /v1/endpoints`, `GET /v1/endpoints`, `DELETE /v1/endpoints/<id>`,
`GET /v1/pull`) carries four headers:

```
X-Relay-Key:       ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAI...   (the full OpenSSH public key line)
X-Relay-Timestamp: 1793000000                                  (unix seconds)
X-Relay-Nonce:     <16 random bytes, base64url, no padding>
X-Relay-Signature: <base64 ed25519 signature over the canonical string below>
```

The full public key rides on every request, not a fingerprint — there's no prior session to look
a fingerprint up against on the very first call (`register` is itself signed). The relay verifies
the signature against the key in the header, then separately decides whether that key is
*authorized* (self-register, licence, `authorized_keys`).

Canonical string, newline-joined, the hash of an empty body for a bodyless `GET`:

```
METHOD "\n" PATH_AND_QUERY "\n" TIMESTAMP "\n" NONCE "\n" SHA256_HEX(BODY)
```

`PATH_AND_QUERY` is the exact request-target as sent (`/v1/pull?batch=50`), so a proxy that
rewrites the query string invalidates the signature instead of silently changing what was asked.

- **Clock-skew window: ±300 seconds.** A timestamp outside that window is `401 stale_timestamp`
  before the signature is even checked — cheaper to reject.
- **Nonce replay**, scoped per key, bounded by the same window — a nonce seen twice is
  `401 replayed_nonce`.
- **`pull` is scoped to the key, not to one endpoint** — one poll per relay answers for every
  endpoint that key owns, not per hook.

## Encryption at rest

The whole record — method, headers, query, body — is sealed as one JSON blob the instant it
arrives, before it's written to disk: `{"method","headers","query","body_b64"}`. What stays in
the clear: `endpoint_id`, `received_at`, `expires_at`, and the plaintext byte length (needed to
enforce body-size and pending-byte limits without ever decrypting).

Envelope, modelled on `adi-mesh/src/lease_seal.rs` (the other place in this codebase anonymously
encrypts to a known X25519 public key over an untrusted hop):

```
ephemeral X25519 keypair, fresh per record, discarded after sealing
ECDH(ephemeral secret, node's static sealing public key)
HKDF-SHA256(ikm = ecdh output, info = "adi-webhook-relay-seal-v1")  →  32-byte key
XChaCha20-Poly1305, random 24-byte nonce, AAD = "<endpoint_id>|<record_id>"
stored: ephemeral_pubkey, nonce, ciphertext
```

The AAD binds a ciphertext to the one record it was sealed for — a row copied or replayed into a
different record's slot fails to decrypt instead of opening as the wrong thing.

**Honest limit.** Plaintext exists in the relay's own process memory from the moment TLS
termination (Caddy, in front of this binary) hands it the request body until sealing completes —
on the same host, never serialized anywhere unsealed, never logged. A compromise of that host's
root, or of the relay or Caddy binary itself, can read a payload *at the instant it arrives*,
indistinguishably from any other reverse proxy anywhere. Encryption at rest protects against a
**later** compromise — a stolen disk, a leaked backup, a database dump — not against a live
attacker on the box at the moment of delivery.

## Wire contract

JSON throughout, snake_case fields. Every non-2xx response:

```json
{ "error": "bad_signature", "message": "human-readable detail" }
```

Codes: `bad_signature`, `stale_timestamp`, `replayed_nonce`, `unknown_key` (self-host: not in
`authorized_keys`), `licence_lapsed` (register only), `rate_limited`, `payload_too_large`,
`too_many_endpoints`, `not_found`, `forbidden` (delete: this endpoint belongs to a different key).

### `POST|GET|PUT|PATCH|DELETE /h/<endpoint-id>` — sender ingress, unauthenticated

Any method is accepted and stored verbatim — the relay doesn't interpret the request, it captures
it, the same stance `/api/hooks/<name>` already takes.

- `202 { "ok": true }` immediately.
- `404 not_found` — unknown, deleted, or never-registered endpoint id.
- `413 payload_too_large` — over the mode's `max_body_bytes`.
- `429 rate_limited` — over the endpoint's `max_requests_per_minute_per_endpoint`, or its pending
  record cap.
- A paused endpoint (enterprise licence lapsed) still answers `202` and still queues — refusing
  intake the moment a licence lapses would make a sender's own retry/alerting treat a billing
  hiccup as an integration failure.

### `POST /v1/endpoints` — register

```json
// →
{ "sealing_key": "<base64 X25519 public key>", "retention_secs": 300, "licence_code": "…" }
// ← 201
{ "endpoint_id": "wh_7f3kq9m2m2x8qr4n", "retention_secs": 300, "url": "https://eu.webhooks.withadi.dev/h/wh_7f3kq9m2m2x8qr4n" }
```

`retention_secs` optional (defaults per mode, below); `licence_code` required only in enterprise
mode. The relay mints `endpoint_id` — `"wh_" + 22 random lowercase-base32 characters` (~110 bits),
since unguessability is the sender's entire protection.

- `401 bad_signature` / `stale_timestamp` / `replayed_nonce`.
- `402 licence_lapsed` — enterprise only.
- `403 unknown_key` — self-host only.
- `429 too_many_endpoints` — over `max_endpoints_per_key`, or (public only) over
  `max_registrations_per_ip_per_day`.

### `GET /v1/endpoints` — list

Signed, no body. Every endpoint the caller's key owns:

```json
{ "endpoints": [
  { "endpoint_id": "wh_7f3kq9m2m2x8qr4n", "retention_secs": 300, "paused": false,
    "created_at": 1793000000, "pending": 3 }
] }
```

### `DELETE /v1/endpoints/<id>` — delete

Signed. `403 forbidden` if the caller's key doesn't own this endpoint (not `404` — the id itself
isn't the secret, the key is). Deletes the endpoint and every queued record immediately. `204` on
success, `404` on an id that never existed or was already deleted.

### `GET /v1/pull?batch=<n>` — long-poll, the node's only read path

Signed, no body. `batch` optional, clamped to `pull_batch_max`. Waits up to
`pull_long_poll_secs` (25s by default) for at least one record across every endpoint the caller's
key owns, returning immediately if one is already queued.

```json
{ "records": [
  { "id": 4821, "endpoint_id": "wh_7f3kq9m2m2x8qr4n", "received_at": 1793000012,
    "sealed": { "ephemeral_pubkey": "…", "nonce": "…", "ciphertext": "…" } }
], "has_more": false }
```

`id` rides the wire because the sealed envelope's AAD is `"<endpoint_id>|<record_id>"` — the
opener has to rebuild that exact string to decrypt, so it needs the id back.

**Delete-at-handout, no ack, no lease.** Every record in a `200` response is deleted from the
relay's storage in the same transaction that selected it. There is no second copy anywhere to
retry from. A node that crashes after the `200` lands but before it finishes replaying every
record in the batch loses whatever it hadn't replayed yet, permanently — a deliberate simplicity
trade, no ack protocol, no lease table, no redelivery logic. An empty `"records": []` means the
poll window elapsed with nothing queued; a node's poll loop treats that identically to "go around
again", never as an error.

### `GET /healthz` — unauthenticated

`200 OK`, plain text — for Docker's own `HEALTHCHECK` and whatever uptime monitor watches the box.

## `relay.toml`

One file, one mode:

```toml
mode     = "public"                      # public | enterprise | self-host
bind     = "0.0.0.0:8080"                # Caddy fronts this; the relay itself never terminates TLS
db_path  = "/data/relay.db"
hostname = "eu.webhooks.withadi.dev"     # used to build the "url" field in a register response

[limits]
max_endpoints_per_key                = 5
max_body_bytes                       = 262144     # 256 KiB
max_requests_per_minute_per_endpoint = 60
max_registrations_per_ip_per_day     = 10
default_retention_secs               = 300
max_retention_secs                   = 900        # 15 min
pull_batch_max                       = 50
pull_long_poll_secs                  = 25

[licence]                                # enterprise only
service_url          = "https://licence.withadi.dev"
public_key            = "<base64 ed25519 public key the licence service signs verdicts with>"
recheck_interval_secs = 86400

[self_host]                              # self-host only
authorized_keys_path = "/etc/webhook-relay/authorized_keys"
shared_secret_env    = "RELAY_SHARED_SECRET"   # name of an env var; omit = not required
ip_allowlist          = []                      # CIDRs; omit/empty = unrestricted
```

Every field under `[limits]` is optional — an absent one resolves to its mode's own default
(`Config::defaults_for` in `src/config.rs`):

| | public | enterprise | self-host (ships as) |
|---|---|---|---|
| `max_endpoints_per_key` | 5 | 100 | unlimited (`u32::MAX`; `authorized_keys` is already the gate) |
| `max_body_bytes` | 256 KiB | 2 MiB | 2 MiB, operator sets |
| `default_retention_secs` / `max_retention_secs` | 300 / 900 | 3600 / 86400 | 300 / unlimited (`u64::MAX`) |
| `max_requests_per_minute_per_endpoint` | 60 | 600 | 600, operator sets |
| `max_registrations_per_ip_per_day` | 10 | n/a (licence-gated instead) | n/a |
| `pull_batch_max` | 50 | 200 | 200, operator sets |

`[licence].public_key` is not optional in enterprise mode and is not part of the task's original
design sketch — it was added because the relay must verify the licence service's beam signature
itself (the same "CLIENT ACCEPTANCE" rule `adi-enterprise-beam-wire-contract-v1` states for every
beam client), and there is nowhere else to configure the key it verifies against.

## Self-hosting

**Self-host mode trusts a list you manage, not a licence or an open door.** Every request has to
be signed by a key listed in your own `authorized_keys` file — there is no self-registration and
no licence check in this mode. You add nodes to the file; a key you remove is locked out
everywhere, not just from registering a new endpoint, the next time it tries.

### What you need

- A machine reachable from the internet on 80/443 (nothing this binary does needs more than one
  core and a few hundred MB of RAM).
- A DNS A record for the hostname you'll run the relay under, pointed at that machine **before**
  you first start Caddy — its ACME http-01 challenge is answered on `:80` by Caddy itself, so
  standing the box up before DNS resolves silently wastes a certificate request.
- Docker and Docker Compose. (Docker is not installed on the box this image is written from —
  build and smoke-test it yourself with `docker compose up` as your own first step.)
- Any `adi-mono` with ADI-MONO-223 landed, for `adi-mono triggers relay-key` and the webhook
  trigger editor's relay picker.

### `relay.toml` for self-host

```toml
mode     = "self-host"
bind     = "0.0.0.0:8080"
db_path  = "/data/relay.db"
hostname = "hooks.example.com"

[limits]
max_body_bytes                       = 2097152      # 2 MiB
max_requests_per_minute_per_endpoint = 600
default_retention_secs               = 300
pull_batch_max                       = 200
pull_long_poll_secs                  = 25

[self_host]
authorized_keys_path = "/data/authorized_keys"
shared_secret_env    = "RELAY_SHARED_SECRET"   # name of an env var; omit the whole line for none
ip_allowlist          = ["203.0.113.0/24"]      # CIDRs or bare addresses; omit/empty = unrestricted
```

Leave `max_endpoints_per_key` and `max_retention_secs` out entirely for "unlimited"
(`authorized_keys` is already the gate) — their literal unlimited values are `u32::MAX`/`u64::MAX`,
which TOML's integer type can't hold written out as a sentinel, so there is no way to spell them
in the file; omission is the only way to get them. This exact sample is a regression test
(`config::tests::self_host_guides_sample_config_parses_and_resolves_unlimited_by_omission`
in `src/config.rs`) — if this README drifts from the real schema, that test fails.

### `authorized_keys`

The exact OpenSSH `authorized_keys` format — one `ssh-ed25519 AAAA… [optional comment]` per line,
`#`-prefixed lines and blanks ignored. **This is a different key than SSH's own** — the relay
reads this file for its own purpose and never touches your SSH daemon's. Reloaded by file mtime,
at most every 5 seconds, so adding a line takes effect without a relay restart; removing one locks
that node out of everything (not just new registrations) within the same window.

```console
$ adi-mono triggers relay-key
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK8LW5yteVrsssWAobt5sR3D6BWU7mkTtx1wrzh5E3H9

$ echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK8LW5yteVrsssWAobt5sR3D6BWU7mkTtx1wrzh5E3H9 my-laptop' \
    >> authorized_keys
```

One key per node (not per trigger), generated on first use and stored at
`~/.adi/mono/relay/{signing,sealing}.key`.

### Docker Compose

`Dockerfile` is a multi-stage build: `rust:1-slim-bookworm` compiles the binary (rusqlite's
`bundled` feature needs a C toolchain for SQLite's own source), the result ships in a
`debian:bookworm-slim` runtime image running as a non-root `relay` user, with `HEALTHCHECK`
against `/healthz` and a `VOLUME /data` for the database.

**Use a named volume for `/data`, not a host bind-mount.** The container's `relay` user is a
Debian system account assigned whatever uid `useradd --system` picks at build time — not a fixed,
predictable number across rebuilds — so a host directory bind-mounted at `/data` would need its
ownership guessed and re-applied after every rebuild. A named volume has no such problem.
`relay.toml` itself is small, operator-edited, and read-only to the container, so it bind-mounts
cleanly regardless of the volume's ownership.

```yaml
# docker-compose.yml
services:
  relay:
    build:
      context: .                                    # the repo root
      dockerfile: crates/adi-relay-server/Dockerfile
    restart: unless-stopped
    ports:
      - "127.0.0.1:8080:8080"                        # Caddy (below) is the only public listener
    volumes:
      - relay-data:/data                             # the database; named, not bind-mounted
      - ./relay.toml:/data/relay.toml:ro              # your own edited copy, read-only
      - ./authorized_keys:/data/authorized_keys:ro    # same — edit on the host, no rebuild needed
    environment:
      RELAY_SHARED_SECRET: "${RELAY_SHARED_SECRET:?}" # only if relay.toml's self_host names one

  caddy:
    image: caddy:2
    restart: unless-stopped
    ports:
      - "80:80"
      - "443:443"
    volumes:
      - ./Caddyfile:/etc/caddy/Caddyfile:ro
      - caddy-data:/data
      - caddy-config:/config
    depends_on:
      - relay

volumes:
  relay-data:
  caddy-data:
  caddy-config:
```

```caddyfile
# Caddyfile — the DNS A record for this hostname must already point here before Caddy's first
# start, because its ACME challenge is answered on :80 by Caddy itself.
hooks.example.com {
    reverse_proxy relay:8080
}
```

Omit the `environment:` block entirely if `relay.toml`'s `[self_host]` names no
`shared_secret_env` — an empty or unset value for a configured one fails every request closed
(a `500`, not a silently-skipped check), so don't half-configure it.

```console
$ docker compose up -d
$ curl http://127.0.0.1:8080/healthz          # the container, directly
ok
$ curl https://hooks.example.com/healthz       # through Caddy, with the real certificate
ok
```

### The optional defence-in-depth layers

Both are **in addition to** the signature check, never instead of it:

- **`shared_secret_env`** — an extra `X-Relay-Shared-Secret` header every signed request must
  also carry, matching the named environment variable's value exactly (compared in constant
  time). Not part of the signed canonical string — a static value a signature already protects
  everything it can protect would gain nothing from also being inside it.
- **`ip_allowlist`** — CIDRs or bare addresses; a connecting address outside every entry is
  refused the same way an unauthorized key is.

Neither is required. If you don't set them, every signed request from a key in `authorized_keys`
is accepted — the file is already the gate this mode leans on.

### The honest limit: delete-at-handout

**A queued record is deleted from the relay's own storage in the exact same instant your node's
`pull` call returns it — not after your node acknowledges having processed it, because there is
no acknowledgement step at all.** A record is gone the instant your node's pull claims it, whether
or not the trigger it was meant for actually ran. A node that crashes mid-batch, a trigger
disabled between registration and delivery, a `trigger_on` project mismatch, your own node's
`/api/hooks` momentarily unreachable — every one of these loses that one record identically,
because the relay has already deleted its only copy by the time any of those would be discovered.
Retention only bounds how long an **unclaimed** record waits for a `pull` that never comes; it
does nothing once a `pull` has already claimed one.

If this loss window matters more to your use case than the simplicity buys you: keep retention
short enough that a sender's own retry (most webhook senders — Stripe, GitHub — retry a non-2xx
or slow response on their own schedule) covers the gap, and keep your node's poller reachable and
unstarved, since every second it isn't is a second of records it will never get a second chance at.

### Rotating or revoking a node's key

There is no key-rotation verb — a relay has no account to rotate a key *within*, since the public
key **is** the identity. To revoke a node: delete its line from `authorized_keys`. It is locked
out of everything within the file's own 5-second reload window, not just new registrations. To
give it a new key: delete `~/.adi/mono/relay/{signing,sealing}.key` on that node (the next
`triggers relay-key` or webhook-trigger save mints a new pair), add the new public key to
`authorized_keys`, and re-save every webhook trigger that named this relay so each gets a fresh
`endpoint_id` under the new key. Every sender that was given the old `/h/<endpoint-id>` URL needs
the new one — there is no way around repasting it.

## Our hosted deploy

`scripts/deploy-webhook-relay-aws.sh <region>` brings up one region (`eu-central-1` or
`us-east-1`) on AWS: one `t4g.small` EC2 instance, two Docker containers (`relay-public`,
`relay-enterprise`, each its own `relay.toml`/db volume/hostname), Caddy in front terminating TLS
and routing by hostname. No SSH — the instance has only an SSM-managed instance profile, and its
security group opens tcp 80/443 only; the script ships the release as a `git archive` through S3
and builds the Docker image on the instance itself over `aws ssm send-command`
(`crates/adi-relay-server/deploy/provision.sh`). DNS (Cloudflare) is set before the instance's
Caddy ever starts, for the same ACME-ordering reason self-host has to care about it. Idempotent
and keyed on `Project=adi-webhook-relay` tags, so a re-run after a later commit ships a fresh
release to the same machine.

Hostnames: `eu.webhooks.withadi.dev`, `us.webhooks.withadi.dev`, `eu.ent.webhooks.withadi.dev`,
`us.ent.webhooks.withadi.dev`. No cross-region anything — each region's SQLite file is local to
that one machine's volume, and nothing in this design replicates, forwards, or compares data
between regions.

## Threat model and honest limits

- **A relay operator (including us) can read a payload for the instant it takes to seal it**, and
  can always deny service. Nothing in this design protects against a relay that chooses not to
  cooperate; the only promise is that a *passive* compromise of the relay's storage yields
  nothing but ciphertext it cannot open.
- **The relay can correlate, even sealed.** Endpoint ids, timing, sender IPs, and request sizes
  are all visible to the relay operator regardless of sealing — sealing hides content, not
  traffic shape.
- **An unguessable endpoint id is the sender's entire defence**, and it is exactly as leaky as
  whatever the sender does with the URL. The relay cannot protect against a leaked URL; only the
  node's own verification of the sender's own signature (e.g. `X-Hub-Signature-256`) can tell a
  replayed capture from the real thing once the id is known — this relay never holds a sender's
  own secret, by design.
- **A compromised node signing key** can register new endpoints and read anything already queued
  for the node's existing ones, until the key is rotated — which means re-registering every
  legitimate endpoint under the new key too.
- **Self-host's `authorized_keys` is only as good as the file it reads** — the exact failure mode
  real SSH `authorized_keys` files have always had.
