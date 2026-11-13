# Self-hosting the webhook relay

For an operator running their own relay instance rather than using one of the hosted ones
(`eu.webhooks.withadi.dev` and friends — see `docs/webhook-relay.md` §9). This is the customer-facing
counterpart to that document; it assumes you've read `docs/webhook-relay.md` §1 (why this exists)
and §6 (the three modes) but explains nothing else from scratch twice.

**Self-host mode trusts a list you manage, not a licence or an open door.** Every request has to
be signed by a key listed in your own `authorized_keys` file (§6) — there is no self-registration
and no licence check in this mode. You add nodes to the file; a key you remove is locked out
everywhere, not just from registering a new endpoint, the next time it tries.

## 1. What you need

- A machine reachable from the internet on 80/443 (a VPS, a small cloud instance — nothing this
  binary does needs more than one core and a few hundred MB of RAM).
- A DNS A record for the hostname you'll run the relay under, e.g. `hooks.example.com`, pointed at
  that machine **before** you first start Caddy (see §4 — this is the one ordering mistake that
  silently wastes a certificate request).
- Docker and Docker Compose on that machine. (Docker was not available on the box this image was
  built from — the image below is written and described in full but not build-verified here; build
  and smoke-test it yourself with `docker compose up` as your own first step, not a later one.)
- The webhook-relay-enabled `adi-mono` on every node that will register an endpoint against this
  relay — any build with ADI-MONO-223 landed has the `triggers` command's `relay-key` verb and the
  webhook trigger editor's relay picker.

## 2. `relay.toml`

One file, one mode. Self-host's own section (`docs/webhook-relay.md` §6's sample, repeated here
with every field explained):

```toml
mode     = "self-host"
bind     = "0.0.0.0:8080"              # Caddy fronts this; the relay itself never terminates TLS
db_path  = "/data/relay.db"            # SQLite — this file is the whole database
hostname = "hooks.example.com"         # used to build the "url" field a register response echoes

[limits]                                # every field optional — shown here are self-host's own
                                         # defaults for the ones worth setting explicitly; leave
                                         # max_endpoints_per_key and max_retention_secs out
                                         # entirely for "unlimited" (authorized_keys is already
                                         # the gate) — TOML's own integer type cannot hold the
                                         # literal u32::MAX/u64::MAX sentinel those two default to
max_body_bytes                      = 2097152      # 2 MiB
max_requests_per_minute_per_endpoint = 600
default_retention_secs              = 300
pull_batch_max                      = 200
pull_long_poll_secs                 = 25

[self_host]
authorized_keys_path = "/data/authorized_keys"
# Both of the next two are optional, off by default, and *in addition to* the signature check —
# never a substitute for it (docs/webhook-relay.md §6).
shared_secret_env    = "RELAY_SHARED_SECRET"   # name of an env var; omit the whole line for none
ip_allowlist          = ["203.0.113.0/24"]      # CIDRs or bare addresses; omit/empty = unrestricted
```

`max_endpoints_per_key`/`max_retention_secs`'s "unlimited" values are `u32::MAX`/`u64::MAX` — the
literal numbers above, not a sentinel the relay treats specially. You're welcome to set a real cap
on either if you'd rather bound one node's endpoint count or a record's shelf life than trust
`authorized_keys` alone to be the only limit.

## 3. `authorized_keys`

The exact OpenSSH `authorized_keys` format — one `ssh-ed25519 AAAA… [optional comment]` per line,
`#`-prefixed lines and blanks ignored. **This is a different key than SSH's own** — the relay
reads this file for its own purpose and never touches your SSH daemon's. Reloaded by file mtime,
at most every 5 seconds, so adding a line takes effect without a relay restart; removing one locks
that node out of everything (not just new registrations) within the same window.

Get a node's public key:

```console
$ adi-mono triggers relay-key
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK8LW5yteVrsssWAobt5sR3D6BWU7mkTtx1wrzh5E3H9
```

One key per node (not per trigger — `docs/webhook-relay.md` §2, §10), generated on first use and
stored at `~/.adi/mono/relay/{signing,sealing}.key`; this same value is what the trigger editor's
panel shows under "copy public key" once a webhook trigger has a relay assigned. Append it to
`authorized_keys` on the relay host:

```console
$ echo 'ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIK8LW5yteVrsssWAobt5sR3D6BWU7mkTtx1wrzh5E3H9 my-laptop' \
    >> authorized_keys
```

The trailing comment (`my-laptop` above) is exactly that — a comment, ignored by the parser,
there so a file with several nodes in it stays readable by whoever maintains it.

## 4. Docker Compose

`crates/adi-relay-server/Dockerfile` is a multi-stage build: `rust:1-slim-bookworm` compiles the
binary (rusqlite's `bundled` feature needs a C toolchain for SQLite's own source; nothing else
in this otherwise-pure-Rust binary does), the result ships in a `debian:bookworm-slim` runtime
image running as a non-root `relay` user, with `HEALTHCHECK` against `/healthz` and a `VOLUME
/data` for the database.

**Use a named volume for `/data`, not a host bind-mount, for the database directory.** The
container's `relay` user is a Debian system account assigned whatever uid `useradd --system` picks
at build time — not a fixed, predictable number across rebuilds — so a host directory bind-mounted
at `/data` would need its ownership guessed and re-applied by hand after every rebuild. A named
volume has no such problem: Docker initializes it from the image's own `/data` ownership once, and
every write after that just works. `relay.toml` itself is small, operator-edited, and read-only to
the container, so it bind-mounts cleanly as a single file regardless of the volume's ownership.

```yaml
# docker-compose.yml
services:
  relay:
    build:
      context: .                                    # the repo root — see the Dockerfile's own note
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
                                                       # (the image's own HEALTHCHECK needs no entry
                                                       # here — Dockerfile already declares it)

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
# Caddyfile — the DNS A record for this hostname must already point here (see §1) before Caddy's
# first start, because its ACME challenge is answered on :80 by Caddy itself.
hooks.example.com {
    reverse_proxy relay:8080
}
```

Omit the `environment:` block entirely if `relay.toml`'s `[self_host]` names no
`shared_secret_env` — an empty or unset value for a configured one fails every request closed
(a `500`, not a silently-skipped check), so don't half-configure it.

Bring it up, then confirm both the health endpoint and the TLS front door:

```console
$ docker compose up -d
$ curl http://127.0.0.1:8080/healthz          # the container, directly
ok
$ curl https://hooks.example.com/healthz       # through Caddy, with the real certificate
ok
```

## 5. The optional defence-in-depth layers

Both are **in addition to** the signature check, never instead of it — a request that fails
either of these never even reaches the signature verification's own 401/403 distinction, it is
simply refused.

- **`shared_secret_env`** — an extra `X-Relay-Shared-Secret` header every signed request must
  also carry, matching the named environment variable's value exactly (compared in constant
  time, so response timing can't be used to guess it). It is **not** part of the signed canonical
  string (`docs/webhook-relay.md` §3) — a static value a signature already protects everything a
  signature can protect would gain nothing from also being inside it.
- **`ip_allowlist`** — CIDRs or bare addresses (`"203.0.113.0/24"`, `"203.0.113.9"`); a connecting
  address outside every entry is refused the same way an unauthorized key is.

Neither is required. If you don't set them, every signed request from a key in `authorized_keys`
is accepted — the file is already the gate the design leans on in this mode.

## 6. The honest limit: delete-at-handout

**A queued record is deleted from the relay's own storage in the exact same instant your node's
`pull` call returns it — not after your node acknowledges having processed it, because there is no
acknowledgement step at all (`docs/webhook-relay.md` §5.5).** This is deliberately simple — no ack
protocol, no lease table, no redelivery logic — and it means a record is gone the instant your node
acknowledges the pull, **whether or not the trigger it was meant for actually ran.** A node that
crashes mid-batch, a trigger disabled between registration and delivery, a `trigger_on` project
mismatch, your own node's `/api/hooks` momentarily unreachable — every one of these loses that one
record identically, because the relay has already deleted its only copy by the time any of those
would be discovered. Retention (§2 above) only bounds how long an **unclaimed** record waits for a
`pull` that never comes; it does nothing once a `pull` has already claimed one.

If this loss window matters more to your use case than the simplicity buys you, the two levers you
actually have are: keep retention short enough that a sender's own retry (most webhook senders —
Stripe, GitHub — retry a non-2xx or slow response on their own schedule) covers the gap, and keep
your node's poller's replay path (`docs/webhook-relay.md` §10) reachable and unstarved, since
every second it isn't is a second of records it will never get a second chance at.

## 7. Rotating or revoking a node's key

There is no key-rotation verb in v1 (`docs/webhook-relay.md` §2, §14 Q3) — a relay has no account
to rotate a key *within*, since the public key **is** the identity. To revoke a node: delete its
line from `authorized_keys`. It is locked out of everything within the file's own 5-second reload
window, not just new registrations. To give it a new key: generate a fresh pair (there's no CLI
verb for this either — delete `~/.adi/mono/relay/{signing,sealing}.key` on that node and the next
`triggers relay-key` or webhook-trigger-with-a-relay save mints a new one), add the new public key
to `authorized_keys`, and re-save every webhook trigger that named this relay so each gets a fresh
`endpoint_id` under the new key. Every sender that was given the old `/h/<endpoint-id>` URL needs
the new one — there is no way around repasting it, which is the real cost of this design decision,
stated plainly rather than discovered later.
