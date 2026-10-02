# adi-channel-router

The channel router: one ADI agent, reachable from Telegram, Slack, and whatever comes after
them. Deployed as a Cloudflare **Worker** (not Pages -- see "Decisions taken" #1 in
docs/channels.md) on **`hooks.withadi.dev`**. It holds the bot credentials, verifies each
service's webhook signature, maps the service's routing key (chat id, team id, ...) to a
subscribed node, and forwards the event over that node's own outbound WebSocket -- queuing it
if the node is offline.

```
Telegram / Slack ──webhook──▶  channel-router (this)
                                  │  verifies the service's signature
                                  │  routing key (chat id, team id, …) → node
                                  ▼
                                Durable Object "NodeConnection"
                                  │  outbound WebSocket, held open by the node
                                  │  queues while the node is offline
                                  ▼
                                node (adi-channels crate, ADI-MONO-121)
```

Full design: [`docs/channels.md`](../../docs/channels.md). This app builds §1 (routes and
storage), §2 (the WebSocket protocol) and §4 (the adapter interface, Telegram only -- Slack is
a registry stub, see below).

Same conventions as its sibling [`apps/oauth-router`](../oauth-router): a provider registry of
public facts, credentials from env keyed on the provider id, signed expiring tokens, small
modules, vitest. It differs from oauth-router in one load-bearing way: it needs a **Durable
Object** per node to hold that node's outbound socket and offline queue, which Pages Functions
cannot hold -- so this one is a Worker, with its own D1 database for the one lookup that has to
happen before a node is even known.

## Routes

| Route | Method | Who calls it | What it does |
| --- | --- | --- | --- |
| `GET /` · `GET /health` | GET | anyone | JSON: service name, enabled providers. |
| `POST /webhook/<provider>` | POST | the service | Verify the service's signature, resolve the routing key to a node via the `routing_keys` D1 table, forward the event to that node's `NodeConnection` Durable Object. A `/start <code>` (Telegram) is a link attempt, not an ordinary message -- consumed here, never reaches `normalize`. Always `200`s once the signature checks out: delivery to the node isn't on this request's critical path. |
| `GET /link/<provider>` | GET | the service's own install flow | A real redirect-based link hop (Slack's "Add to Slack"). Telegram links through its webhook instead, so this 404s for every provider built so far. |
| `POST /subscribe` | POST | a node | Upgrade to WebSocket. `Authorization: Bearer <node-token>` (or `?token=`) identifies the `(node, provider)` Durable Object this socket belongs to. |
| `POST /send` | POST | a node | `{ connection, text }`. Resolves the connection's chat/team id from its Durable Object and posts through the provider's own adapter with this deployment's credential -- the only path a reply takes; a bot token never reaches a node. |
| `POST /register` | POST | a node, holding `ROUTER_ADMIN_SECRET` | Mint a node token (first call for a `(node, provider)` pair only -- later calls reuse it) plus a fresh pending connection and a signed link code. |
| `POST /disconnect` | POST | a node | `{ connection }`. Drops the connection from its Durable Object and the D1 routing row, and revokes the node token too once nothing else on this `(node, provider)` still needs it (ADI-MONO-122, closing the gap flagged below). |

## Providers

The registry is [`src/providers.ts`](src/providers.ts); adapters are one file per service under
[`src/adapters/`](src/adapters/).

- **`telegram`** -- the only working adapter. `TELEGRAM_BOT_TOKEN` + `TELEGRAM_SECRET_TOKEN`
  enable it; `TELEGRAM_BOT_USERNAME` (public) builds `t.me/<bot>?start=<code>` install links.
- **`slack`** -- named in the registry, `enabled()` always `false`. No adapter file exists yet
  (ADI-MONO-123); every route that needs one 404s for it until then.

## Configuration

| Name | Kind | Purpose |
| --- | --- | --- |
| `ROUTER_SECRET` | secret | HMAC key signing node tokens and link codes. `openssl rand -hex 32`. |
| `ROUTER_ADMIN_SECRET` | secret | Bearer gating `POST /register`. A node holds this too (docs/channels.md §8). |
| `TELEGRAM_BOT_TOKEN` | secret | The one Telegram bot ADI runs. |
| `TELEGRAM_SECRET_TOKEN` | secret | `secret_token` set at `setWebhook` time; echoed back on every delivery. |
| `TELEGRAM_BOT_USERNAME` | var | Public -- builds the `t.me/<bot>?start=<code>` install link. |
| `NODE_CONNECTION` | Durable Object binding | One `NodeConnection` object per `(node, provider)` pair. |
| `ROUTING_KEYS` | D1 binding | `provider, routing_key -> node_id`. The only lookup that runs before a node is known. |

Vars live in [`wrangler.toml`](wrangler.toml); secrets are set with `wrangler secret put <NAME>`
and never committed.

## Develop

```bash
cd apps/channel-router
bun install

cp .dev.vars.example .dev.vars   # fill in local secrets (git-ignored)
bun run dev                      # wrangler dev

bun run typecheck                # tsc --noEmit
bun run test                     # vitest, inside the real Workers runtime (miniflare)
```

The suite runs through `@cloudflare/vitest-pool-workers`, not plain Node -- unlike
oauth-router's logic, this router's correctness is inseparable from two Workers-only
primitives (Durable Objects, D1), so the tests need the real runtime underneath them, reading
its bindings straight from `wrangler.toml` (see [`vitest.config.ts`](vitest.config.ts)). D1
migrations ([`migrations/`](migrations/)) are applied once per test file via `beforeAll` (see
[`test/migrate.ts`](test/migrate.ts)); per-test storage isolation is turned off
(`isolatedStorage: false`) because a Durable Object that holds a WebSocket open past the
request that created it -- exactly `do.ts`'s subscribe/queue path -- hits a known
incompatibility with that isolation in this pool version. Every test already uses a freshly
randomised node id per connection, so sharing storage across tests within a file costs nothing.

## Standing it up on a Cloudflare account

Not done by this task (ADI-MONO-120 is build-and-test only -- see docs/channels.md §9,
ADI-MONO-124 is the deploy step and needs Igor's sign-off: a real bot, a real domain). When it
happens, it needs, analogous to oauth-router's `scripts/setup-cf.sh`:

1. the Worker `adi-channel-router` deployed, with a `NodeConnection` Durable Object and a
   `routing_keys` D1 database (`wrangler d1 create adi-channel-router-routing-keys`, then the
   real `database_id` into `wrangler.toml`)
2. `ROUTER_SECRET`, `ROUTER_ADMIN_SECRET`, `TELEGRAM_BOT_TOKEN`, `TELEGRAM_SECRET_TOKEN` set as
   Worker secrets
3. `hooks.withadi.dev` attached as a custom domain
4. a real `@AdiBot` registered with Telegram, with `setWebhook` pointed at
   `https://hooks.withadi.dev/webhook/telegram` and a `secret_token` matching
   `TELEGRAM_SECRET_TOKEN`

## Security notes

- **Signed, expiring tokens, no session store** -- same posture as oauth-router's `state`: a
  node token and a link code are both HMAC-signed over `ROUTER_SECRET` and carry their own
  issued-at, verified fresh on every use (see `src/state.ts`).
- **Token scopes.** A node token is scoped to exactly `(node id, provider)` -- it cannot read or
  send on a different node's connections, and a Telegram token cannot open a Slack socket
  (docs/channels.md §8).
- **`ROUTER_ADMIN_SECRET` gates minting, not ordinary traffic.** Only `POST /register` needs it;
  `/subscribe` and `/send` run on the node token alone.
- **Signature checks run before anything else touches a webhook body.** An unverified Telegram
  webhook is refused with a generic `400`, never reaching the Durable Object.
- **Bot credentials never leave the router.** `POST /send` is the only path a reply takes; a
  node never calls Telegram or Slack directly.
- **The allowlist is enforced on the node, not here** (docs/channels.md §5, §8) -- this router
  forwards every webhook for a linked routing key regardless of sender; a bug here can leak a
  message, never grant a reply from an agent a sender isn't allowed to talk to.

## ADI-MONO-122: `/disconnect` and the epoch counter

ADI-MONO-120 shipped without a disconnect route on purpose (see below); ADI-MONO-122 closes
that gap with `POST /disconnect` (auth and shape above) plus one fix it required: `register`'s
epoch was hardcoded to `1` for every freshly minted token, which `state.ts`'s own doc already
promised wasn't the plan ("minting or revoking bumps the epoch the owning Durable Object has on
file") but nothing before this task ever revoked anything to notice. A revoked-but-still-
correctly-signed token would otherwise pass `/internal/token/check` against a connection
registered *after* the revoke, since that check only compares epoch numbers. Fixed with a
durable `epoch_counter` in the Durable Object's own storage that survives a token being deleted
(unlike the token record itself) and always hands out one past the highest epoch this object has
ever minted.

## Corrections made to docs/channels.md while building this

- **The Durable Object key is `(node, provider)`, not `node` alone.** §1 originally said "one
  Durable Object per node, keyed by node id"; §2 already required two sockets through two
  different Durable Objects for a node on both Telegram and Slack, which only both hold if the
  key is the pair. Taken as the pair (also exactly the granularity §8 scopes a node token to).
- **`ChannelAdapter.normalize` is `async` and takes `env`.** The interface in §4 had it
  synchronous and env-less, but Telegram's own worked-through section in the same document
  requires a `getFile` round-trip (an async network call needing the bot token) before
  forwarding a photo attachment -- the two couldn't both be true as written.
- **`ChannelAdapter.extractLinkCode` is new**, not in the original §4 signature. Telegram's
  `/start <code>` arrives as an ordinary webhook (§4 already says so), so the webhook handler
  needs a uniform, optional way to ask "is this a link attempt?" without hard-coding Telegram's
  command syntax into `router.ts`.
- **§1's route table has no `/disconnect`**, even though §7's CLI table calls for one and
  describes what it should do to the router. Left unbuilt here on purpose: ADI-MONO-120's own
  build list (§9) names exactly `/subscribe`/`/send`/`/webhook`/`/link`/`/register` and nothing
  else, so this is flagged for whichever task adds `adi-mono channels disconnect`, not fixed in
  this one.
