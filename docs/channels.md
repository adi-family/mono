# Channels — spec

One ADI agent, reachable from Telegram, Slack, and whatever comes after them. Read this before
touching `apps/channel-router`, the node-side `adi-channels` crate, the `/settings/channels`
panel, or `adi-mono channels`. Tracked as ADI-MONO-118; this document is ADI-MONO-119, and
ADI-MONO-120..124 build against it in the order given at the end.

Status: **design.** Nothing below is built yet.

## The shape, in one picture

```
Telegram / Slack ──webhook──▶  channel-router (Cloudflare Workers)
                                  │  verifies the service's signature
                                  │  routing key (chat id, team id, …) → node
                                  ▼
                                Durable Object "node:<id>"
                                  │  outbound WebSocket, held open by the node
                                  │  queues while the node is offline
                                  ▼
                                node (this machine)
                                  │  adi.channels.message on the bus
                                  │  connection → agent conversation (run or reply)
                                  ▼
                                agent run ──final answer──▶ POST /send (router) ──▶ service
                                         ──mid-run post────▶ channel-reply tool ───┘
```

The router is a sibling of `apps/oauth-router`: same Workers-adjacent conventions (a provider
registry of public facts, credentials from env keyed on the provider id, signed expiring state,
small modules, vitest), different job. oauth-router hands a browser a token and forgets it;
channel-router holds long-lived bot credentials and brokers a standing relationship between a
service's chat/workspace and a node.

**"Node" here means an ADI install** — the same machine that runs `adi-mono`/`adi-app`/the hive.
It is unrelated to a fleet node (`docs/fleet.md`): no mesh, no iroh, no Durable-Object-per-peer
analogy intended beyond the one actually drawn below (both are "whoever is behind NAT dials out
and holds the session open").

## 1. Routes and storage

A new Cloudflare **Worker** (not Pages — it needs Durable Objects, which Pages Functions cannot
hold), `apps/channel-router`, deployed at `hooks.withadi.dev`. Catch-all `fetch` handler, same
"one small module per concern" layout as oauth-router: `router.ts` dispatches, `providers.ts` is
the registry, `state.ts` signs the link code, `do.ts` is the Durable Object, `adapters/*.ts` is
one file per service.

| Route | Method | Who calls it | What it does |
| --- | --- | --- | --- |
| `GET /` · `GET /health` | GET | anyone | JSON: service name, enabled providers. |
| `POST /webhook/<provider>` | POST | the service | Verify the service's signature, normalize the payload, resolve the routing key → node, forward the event over that node's WebSocket (queue if offline). Answers `200` to the service as soon as the webhook is accepted — delivery to the node is not on this request's critical path, because a webhook provider retries on a slow `200` far more aggressively than on a fast one. |
| `GET /link/<provider>?code=<link-code>` | GET | the service's own install flow (Telegram's `/start` deep link; Slack's OAuth redirect) | Verify the signed link code, record the routing key → node binding, tell the node (over its WebSocket) that a new chat/workspace is attached. For Telegram this *is* the bot receiving `/start <code>` as an ordinary message, handled like any other webhook with one extra step; for Slack it is the callback leg of "Add to Slack". |
| `POST /subscribe` | POST | a node | Upgrade to WebSocket. `Authorization: Bearer <node-token>` (query param `?token=` for clients that cannot set a header on the upgrade request) identifies which Durable Object this node is subscribing to. |
| `POST /send` | POST | a node | `{ "connection": "<id>", "text": "…", … }` (§3). Looks up which service/chat the connection maps to from the DO's own state, calls that provider's credential, posts to the service. This is the **only** path a reply takes — a node never calls Telegram/Slack directly, so a bot token never leaves the router. |
| `POST /register` | POST | a human operator (CLI/panel, not a node) | Mint a node token for a new connection (§2). Requires the router's own operator credential (`ROUTER_ADMIN_SECRET`, analogous to `STATE_SECRET`), not a service's. |

**Storage.** One Durable Object per node, keyed by node id — `idFromName(nodeId)`. The DO holds:

- the node's current WebSocket (if connected) and its outbound frame queue (if not);
- every connection this node has (routing key → `{provider, agent/trigger/app-route target,
  allowlist, thread map}`, §5) — a node's connections are its own DO's state, not a global table,
  so a node's data lives with the one object that ever touches it and two nodes never contend;
- the signed, expiring link codes it has minted (§2), so `/link/<provider>` can verify one without
  a second store.

A **D1 table** (`routing_keys`), outside any one DO, is the only thing that has to be queried
*before* a node is known: `provider, routing_key → node_id`. The webhook handler is the one path
that has a routing key and nothing else — it has to find the DO before it can ask the DO
anything. Everything else (connections, allowlists, thread maps) stays inside the owning DO,
reachable once the node id is known. Durable Object storage alone cannot be queried by routing
key without iterating every object, which is the one thing this table exists to avoid.

No KV, no R2. The signing key for link codes and node tokens (`STATE_SECRET`-equivalent, call it
`ROUTER_SECRET`) and each provider's bot credentials are Worker secrets, exactly as oauth-router's
`STATE_SECRET` and `<PROVIDER>_CLIENT_SECRET` are — never in D1, never in the DO's own storage.

### Node subscribe / token lifecycle

1. An operator runs `adi-mono channels connect <svc> --agent <a>` (§7). The node calls
   `POST /register` on the router (bearer `ROUTER_ADMIN_SECRET`, which is itself a secret the node
   holds — see §8 for why this is acceptable) and gets back a **node token**: an opaque,
   signed-and-expiring bearer good for one `(node id, provider)` pair. It is stored locally in
   `adi-secrets` under a reserved scope (the same mechanism a fleet node's password uses — never
   injected into an agent run's environment).
2. The node opens `POST /subscribe` with that token and keeps the WebSocket open (§2 of the wire
   protocol, below) for as long as the connection exists. A dropped socket reconnects with the
   same token; the token does not rotate on reconnect.
3. The *first* inbound message for a not-yet-linked chat is a link code, not ordinary traffic:
   the human who ran `connect` is shown `t.me/<bot>?start=<code>` (or Slack's "Add to Slack" URL,
   whose OAuth `state` carries the same signed code). Following it binds that chat/workspace's
   routing key to this node inside the DO and in the `routing_keys` table. From then on every
   webhook for that routing key forwards to this node.
4. **Revocation** is `adi-mono channels disconnect`: the node tells the router to drop the
   connection (erases the DO's entry and the D1 row) and discards its local copy of the thread
   map. The node token itself is revoked the same call if no other connection on this node still
   needs it — a node holds one token per provider, shared across every connection on that
   provider, not one per connection.

## 2. The node↔router WebSocket protocol

One WebSocket per node per provider (a node talking to both Telegram and Slack holds two
sockets, because each is subscribed through a different Durable Object and losing one must not
touch the other). Every frame is a JSON object with a `type` tag — `{"type": "...", ...}` — kept
flat and small on purpose: this socket carries control and event traffic, never the bytes of an
attachment (those stay on the service's own CDN; see §3).

```ts
// router → node
{ "type": "event",   "id": "evt_...", "message": ChannelMessage }   // §3
{ "type": "linked",  "connection": "<id>", "provider": "telegram", "routing_key": "..." }
{ "type": "ping" }

// node → router
{ "type": "ack",     "id": "evt_..." }
{ "type": "pong" }
```

- **At-least-once delivery, acked.** The router holds `event` frames in the DO's queue until
  `ack`ed by that `id`. A node that disconnects mid-delivery gets the same event again on
  reconnect — the node's own event handling must be idempotent on `id` (keep the last N delivered
  ids; `adi.channels.message`'s payload carries the same `id` as its dedup key).
- **Offline queueing.** While no socket is attached, `event` frames accumulate in the DO's
  storage, capped (same shape as `adi-events`' `MAX_SPOOL`: oldest dropped past the cap, since an
  offline node that was offline long enough to overflow has bigger problems than one dropped
  typing notification). On reconnect the whole queue drains before new traffic, oldest first.
- **Reconnect.** The node's client is a simple backoff loop (1s, 2s, 4s, … capped at 60s) that
  re-opens `POST /subscribe` with the same token. The router does not need to tell the node to
  reconnect — a closed socket is itself the signal.
- **Heartbeat.** The router sends `ping` every 30s on an idle socket; a node that misses two in a
  row is presumed gone and its socket is dropped (freeing the DO to accept a fresh one rather than
  holding a half-dead connection forever). The node answers `pong` and need not send its own pings
  — Cloudflare's own WebSocket idle timeout is the thing being raced here, not application health.
- **Auth.** The bearer node token at `/subscribe` is the entire authentication for the socket;
  nothing inside a frame carries a credential. A frame from the wrong node is impossible by
  construction, since the socket itself is scoped to one DO.
- **`/send` is plain HTTP, not a frame on this socket** — a reply is node-initiated and the socket
  is receive-oriented by convention (simpler to reason about than a socket carrying traffic both
  ways with two different message grammars). `/send` still authenticates with the same node token.

## 3. The provider-neutral message schema (v1)

`ChannelMessage` is what every adapter normalizes *into*, and the only shape anything past the
adapter boundary ever reads.

```ts
interface ChannelMessage {
  v: 1;
  id: string;            // dedup key — stable across redelivery, unique per (provider, id)
  provider: string;      // "telegram" | "slack" | ...
  connection: string;    // the id minted at link time (§5) — which agent/trigger/route this is
  thread: string;        // provider's own thread/chat identity, opaque past the adapter
  sender: {
    id: string;          // provider's user id, stable
    name: string;        // display name or handle, for the From marker (§5)
  };
  text: string;           // already extracted: captions, slash-command args, etc. folded in
  attachments: Array<{
    kind: "image" | "file";
    url: string;          // a short-lived provider URL the node fetches itself — never proxied
                           // through the router or riding the WebSocket as bytes
    name?: string;
  }>;
  reply_to?: string;      // this message's id, if it quoted/replied to an earlier one
  raw_kind: string;       // what the provider actually called it ("message", "edited_message",
                           // "app_mention", …) — for an adapter-specific trigger that needs more
                           // than the normalized shape; never parsed past the adapter itself
  received_at: number;    // epoch ms, router's clock
}
```

This is what rides inside an `event` frame's `message` field (§2) and, unchanged, inside the bus
event's payload:

```
adi.channels.message   — a ChannelMessage, as JSON, emitted by the node's router client the
                          moment a frame is acked. Catalog entry lives beside adi.agents.* (§5
                          names exactly where).
```

**Versioned from day one** (`v: 1`) because this schema is read by code on three different
deploy cadences — the router (deploys independently), the node's client, and whatever trigger or
agent code a user has written against the shape — and "add a field" is the only change v1 ever
promises *not* to break: every consumer must ignore a field it doesn't recognize, and `v` bumps
only on a change that isn't additive.

## 4. The adapter interface

```ts
interface ChannelAdapter {
  id: string;                                               // "telegram", "slack"
  verify(req: Request, env: Env): Promise<boolean>;         // signature check, before anything
                                                             // else touches the body
  routingKey(payload: unknown): string | null;              // chat id / team id — null if this
                                                             // payload carries none (e.g. a
                                                             // Slack url_verification challenge)
  normalize(payload: unknown, connection: string): ChannelMessage[];  // 0, 1, or several —
                                                             // a Slack event batch can carry more
                                                             // than one
  send(reply: OutboundReply, credential: ResolvedProvider): Promise<void>;  // the one place a
                                                             // bot token is actually used
  // Install/link, see below — shaped differently per service, so not one signature.
}
```

`verify` and `routingKey` run before the router does anything else with a request — unlike
oauth-router's state (which the router itself signs), a webhook's authenticity is the *service's*
signature over a body the router did not create, so each adapter owns exactly how that is
checked.

### Telegram, worked through it

- **`verify`**: Telegram has no HMAC header; it has a *secret path segment* instead
  (`setWebhook`'s `secret_token`, echoed back as `X-Telegram-Bot-Api-Secret-Token`). `verify`
  checks that header against `TELEGRAM_SECRET_TOKEN` (env, per-bot, set once at `setWebhook` time)
  in constant time.
- **`routingKey`**: `message.chat.id` (a DM, group, or channel — Telegram calls all three
  "chat").
- **`normalize`**: one `ChannelMessage` per update. `message.text` → `text`; a photo update's
  largest `PhotoSize.file_id` is resolved via `getFile` to a URL (Telegram file URLs expire, so
  this is the one adapter call that round-trips to Telegram *before* forwarding, not after).
- **install/link**: `t.me/<bot_username>?start=<code>` — Telegram delivers `/start <code>` as an
  ordinary `message` update, so `GET /link/telegram` in the route table above doesn't actually
  exist for this provider; the *webhook* handler recognizes a `/start` command, verifies `code`
  the same way `/link/<other-provider>` would, and answers both the chat (a welcome message, sent
  via `send`) and the linking logic in one pass. Slack needs the separate route because its OAuth
  redirect is a real browser hop; Telegram's "link" is just another webhook.
- **`send`**: `POST https://api.telegram.org/bot<token>/sendMessage`, with `TELEGRAM_BOT_TOKEN`
  the per-deployment secret, exactly the convention `<PROVIDER>_CLIENT_SECRET` sets for
  oauth-router.

### Slack, worked through it

- **`verify`**: HMAC-SHA256 over `v0:<timestamp>:<raw body>` with the signing secret, compared to
  `X-Slack-Signature`, and the timestamp rejected past 5 minutes old — Slack's own documented
  scheme, structurally identical to oauth-router's `verifyState` (sign, compare, check age) but
  over the inbound body instead of an outbound token.
- **`routingKey`**: `team_id` (a Slack **workspace**, not a channel — one workspace's agent
  conversation spans every channel the bot is invited into, which is a deliberate difference from
  Telegram's per-chat granularity; §5's thread mapping is what narrows it back down per-channel).
- **`normalize`**: Slack's Events API wraps everything in an envelope; `normalize` unwraps
  `event.type` (`message`, `app_mention`, …) into zero-or-one `ChannelMessage` (a `url_verification`
  challenge yields zero — `routingKey` already returned `null` for it, so it never reaches
  `normalize`) — and **is the one adapter that must answer Slack's own retry/dedup contract**: a
  slow event handler makes Slack re-send the same `event_id` up to 3 times, so `id` in the
  normalized message is Slack's `event_id` verbatim, which is also why `adi.channels.message`'s
  dedup key (§3) matters here specifically, not as a hypothetical.
- **install/link**: "Add to Slack" is OAuth — `GET /link/slack` *is* real here. The signed link
  code rides in OAuth's own `state` param (exactly `oauth-router`'s pattern, reused rather than
  reinvented), and the callback both completes the OAuth token exchange (storing the resulting bot
  token, scoped to that one workspace, in the DO) and binds the routing key.
- **`send`**: `POST https://slack.com/api/chat.postMessage` with the **workspace's own** bot
  token (not a single `SLACK_BOT_TOKEN` the way Telegram has one bot for everyone — Slack's model
  is one app installed into many workspaces, each handing back its own token at OAuth time, so the
  credential lives in the DO's per-connection state, not in a Worker secret keyed by provider id).
  This is the one place Telegram and Slack's credential shapes genuinely differ, and it is why
  `send`'s `credential` parameter is resolved per-connection rather than looked up once per
  provider the way oauth-router's `resolveProvider` does.

## 5. The connection model

A **connection** is the thing a link creates: one `(provider, routing key)` bound to one
**target** and one **allowlist**, living in the owning DO (§1) and mirrored to the node (so the
node can route without a round trip for every message).

```ts
interface Connection {
  id: string;
  provider: string;
  routing_key: string;           // chat id, team id, ...
  target: Target;                // generic — see below
  allowlist: Allowlist;
  threads: Record<string, string>;  // provider thread id → ADI conversation (run) id, §(below)
}

type Target =
  | { kind: "agent";   agent: string }
  | { kind: "trigger"; trigger: string }
  | { kind: "app_route"; app: string; route: string };

type Allowlist =
  | { kind: "owner_only" }                 // default: only whoever ran `connect`/completed the link
  | { kind: "list"; sender_ids: string[] } // named provider user ids, added via `adi-mono channels allow`
  | { kind: "open" };                      // anyone in the chat/workspace — opt-in, never default
```

**Why the target is an enum now, for one variant used today.** Only `agent` is built in
ADI-MONO-121; `trigger` and `app_route` are named so a bundle-installed channel (§6) never has to
widen this type later — only add handling for a variant already on the wire. A connection whose
target an install doesn't recognize is exactly the tolerance `docs/marketplace-bundles.md` already
asks of an unknown element `kind`: skipped, not refused.

**Thread → conversation mapping.** One service thread is one ADI conversation, for the lifetime of
the connection:

- Telegram: `thread` is the chat id itself (no sub-thread concept for a DM or small group; a
  Telegram *topic* inside a forum-mode supergroup is `message_thread_id` and gets its own entry).
- Slack: `thread` is the channel id, or `channel:thread_ts` once a reply opens a Slack thread —
  so a channel's main stream and each of its threads are separate ADI conversations, matching how
  a Slack user actually experiences "a thread" as its own topic.
- The **first** message on a thread not yet in `threads` calls `POST /api/agents/run` against the
  target's agent and records the run id it gets back. **Every later** message on that thread calls
  `POST /api/agents/run/reply` with that run id — i.e. the provider's thread id *is* the connection's
  only state past the first message; the run id it maps to is exactly the "conv" id
  `adi.agents.question.asked`'s payload already treats as a first-class conversation handle.
- **Who said it is stamped with the existing `Marker::From`** (`crates/adi-agents/src/marker.rs`)
  — `node` set to the provider id (`"telegram"`), `user` to the sender's display name — rather
  than a new marker kind. `From` already means exactly this ("who said the words after it, on a
  [surface] more than one person can reach") and the system prompt block that teaches every agent
  about markers needs no change.
- **The run's final answer posts back automatically.** The node subscribes to
  `adi.agents.run.finished` (already published, §"what became of a run" in `guides/agents.md`) for
  every run a connection opened, reads `result_head`-or-better (the full answer, not just the
  head — the connections component keeps its own small index from run id → connection, so it asks
  the run for its answer rather than trusting the notification's truncated field) and calls
  `POST /send` on the router. A run that ends with a question (`adi.agents.question.asked`) posts
  the question back the same way instead of waiting silently.
- **Mid-run posts go through a `channel-reply` tool**, an ordinary `adi-mono tools`-owned script
  (`guides/tools.md`'s pattern — not an engine-native tool like `Ask`/`Report`, so it works
  identically on every backend, not only the harness ones). It reads `ADI_AGENT` / `ADI_RUN_ID`
  from its environment (the same pair `launcher::by_caller` already reads), looks up the
  connection those map to, and calls the node's own small local endpoint that forwards to
  `POST /send` — mirroring `Report`'s shape (announce something mid-turn) without needing the
  harness's suspend/resume machinery `Ask` does, because a channel post, unlike a question, never
  blocks the turn.
- **Allowlist enforcement is on the node, not the router.** The router forwards every webhook for
  a linked routing key regardless of sender; the node's event handler checks `sender.id` against
  the connection's `Allowlist` *before* calling `run`/`reply`, and a disallowed sender gets either
  silence or a configurable "you're not allowed to talk to this agent" reply (default: silence —
  telling a stranger a bot exists and is gated is strictly worse than saying nothing). Keeping the
  check on the node rather than the router is the same locality rule §1 draws for connection state:
  the node is the thing with an agent to protect, the router is a dumb pipe that doesn't know what
  "allowed" means for any agent.

## 6. Apps and marketplace bundles installing a channel later

Nothing here is built now (ADI-MONO-118's scope is Telegram and Slack by hand); this section is
what ADI-MONO-120..123 must not foreclose.

- **`Target::app_route`** (§5) is the whole hook for a dashboard backend to be a channel's
  destination instead of an agent — an app's `backend/routes/*.ts` already answers `/api/...`
  under its own origin (`docs/apps.md` §1), so `app_route` names `{app, route}` and the node's
  dispatcher calls that app's backend the same way it would call `/api/agents/run/reply`, with
  the `ChannelMessage` as the POST body. No new mechanism — the same "one origin, three entry
  points" shape apps already have is enough to receive one.
- **Bring-your-own bot credentials.** `POST /register` (§1) is scoped to `(node, provider)`, not
  hard-coded to "the one Telegram bot ADI runs" — an app that wants its *own* `@MyAppBot` registers
  its own `TELEGRAM_BOT_TOKEN`-equivalent against the router under its own provider-id suffix
  (`telegram:myapp`, mirroring how Slack already needs a per-workspace token in §4, not a single
  shared one). The provider registry entry (`authUrl`/webhook shape) is reused; only the credential
  and the bot's own identity are the app's. This is the generalization oauth-router's own registry
  already models — "public facts in the registry, credentials from env keyed on the id" scales
  from one credential per provider to one per `(provider, app)` without changing the registry's
  shape, only its key.
- **Channels as a marketplace element kind.** `docs/marketplace-bundles.md`'s eight kinds
  (`agent | tool | dashboard | llm | embedding | service | trigger | project`) gain a ninth,
  `channel`, whose repository layout entry is `channels/<name>.toml` — a `Connection`-shaped file
  *minus* `routing_key` and `threads` (those are facts about a specific install, never a
  publisher's to ship, exactly the store-owned-fields rule that document already states for every
  other kind). Installing one lands a **disconnected** connection naming a target and an
  allowlist default; "inert on arrival" for this kind means exactly what it means for a trigger
  (§"Inert on arrival, per kind" in that document) — present, but not linked to any real chat
  until the operator runs `connect` against it, which is the bundle case of the same flow §7
  already has to support for a hand-created one.
- **Nothing above is a reason to add a ninth kind today.** It is named here so ADI-MONO-120's
  `Connection`/`Target` shapes don't have to be revisited when it happens — the whole point of
  pinning this now.

## 7. UI and CLI, mapped to the node's own endpoints

The node exposes a small API (`adi-webapp-api`, alongside `/api/agents/*`) that both the panel and
the CLI call — neither talks to the router directly except where noted.

| Panel (`/settings/channels`) | CLI | Node endpoint | Router call it makes, if any |
| --- | --- | --- | --- |
| A card per service, "Connect" | `adi-mono channels connect <svc> --agent <a>` | `POST /api/channels/connect` `{provider, target}` | `POST /register` (mint a node token if this node has none yet for that provider), then opens `/subscribe` if not already open |
| — picks agent + who may talk, then shows the install/link URL and waits | (same command prints the URL and blocks until linked) | `GET /api/channels/<id>` polled for `linked: true` | none — the node is just waiting on its own WebSocket for a `linked` frame (§2) |
| Connected row: Change agent | `adi-mono channels route <id> --agent <a>` | `POST /api/channels/route` `{id, target}` | none — target lives in the node's mirror and the DO; the node pushes the change up over the socket |
| Pause | *(part of `route`/`disconnect`; no separate verb — see "Decisions taken")* | `POST /api/channels/pause` `{id, paused}` | none |
| Who may talk | `adi-mono channels allow <id> --add <sender>` / `--mode open\|owner_only` | `POST /api/channels/allow` `{id, allowlist}` | none |
| Disconnect | `adi-mono channels disconnect <id>` | `POST /api/channels/disconnect` `{id}` | tells the router to drop the DO entry and the D1 row; revokes the node token if nothing else on this node needs it |
| *(row listing)* | `adi-mono channels list` | `GET /api/channels` | none |

`list`/`connect`/`route`/`allow`/`disconnect` is exactly the CLI verb set the parent task names;
nothing here invents a sixth. "Pause" folds into `route`/a dedicated flag rather than earning a
new endpoint, because pausing is "stop delivering to the target" — a property of the connection's
target handling, not a new axis the store needs to track twice.

## 8. Security

- **Token scopes.** A node token (§1) is scoped to exactly `(node id, provider)` — it cannot read
  or send on a different node's connections, and a Telegram token cannot open a Slack socket. It
  is a bearer, not a signed claim the node can inspect; the router is the only party that resolves
  it, same posture as a session cookie.
- **`ROUTER_ADMIN_SECRET` gates minting, not ordinary traffic.** Only `POST /register` needs it;
  `/subscribe` and `/send` run on the node token alone, so a compromised node token exposes one
  node's connections, never the ability to mint more.
- **Signature checks are mandatory and provider-specific** (§4) and run before `normalize` touches
  the body — an unverified webhook is refused with the generic `400` a provider's retry logic
  already knows how to handle, never shown to the DO.
- **What the router may store, and what it may not.** Bot/app credentials: yes, as Worker secrets
  (Telegram) or per-connection DO state (Slack's per-workspace token) — this is the entire point
  of the router existing, so a node never holds a bot token. `ChannelMessage` bodies: only
  transiently, in the DO's redelivery queue, until acked; never written to D1. **Slack is
  zero-copy on top of that**: Slack's own retention/DLP posture for an "Agents & AI Apps" listing
  expects the platform to not durably store message content it didn't need to — the DO's queue
  already satisfies "transient, not durable" for every provider, and Slack gets nothing extra
  held beyond that window. User PII beyond `sender.id`/`sender.name` (§3) is never requested from
  any provider and so never flows through the router at all.
- **Revocation** (§1) is immediate on the router side (DO entry and D1 row gone the moment
  `disconnect` returns) and best-effort on the service side — Telegram keeps delivering to the
  webhook URL regardless of any one chat's link, so "revoked" means "this routing key no longer
  resolves to a node," answered with a `404`-equivalent the adapter swallows, not "the bot leaves
  the chat." Slack's workspace-level OAuth token is explicitly revoked via Slack's own
  `auth.revoke` at disconnect time, since Slack (unlike Telegram) gives the router something
  capable of being revoked.
- **The allowlist is enforcement, not secrecy.** §5 already places the check on the node; stated
  again here because it is the one access-control decision in this whole design that is not the
  router's to make or bypass — a bug in the router can leak a *message*, never grant someone a
  reply from an agent they aren't allowed to talk to, since the node never calls `run`/`reply`
  for a disallowed sender in the first place.

## 9. Build order: ADI-MONO-120 .. 124

1. **ADI-MONO-120 — the router.** `apps/channel-router`: Workers + Durable Object, the Telegram
   adapter, `/subscribe`/`/send`/`/webhook`/`/link`/`/register`, vitest over all of it. No
   deploy — `wrangler dev` and the test suite are the whole verification surface for this task.
2. **ADI-MONO-121 — the node side.** The `adi-channels` crate: the WebSocket client (reconnect,
   ack, dedup by `id`), `adi.channels.message` on the bus (catalog entry alongside `adi.agents.*`,
   assembled one layer above `adi-agents` since `adi-channels` depends on it — see
   `crates/adi-agents/src/events.rs`'s own note on why the catalog lives at the lowest crate that
   can see every producer), the connection store, the thread→run mapping, the
   `channel-reply` tool, and the automatic post-back on `adi.agents.run.finished`. This is the
   task that makes Telegram usable end-to-end against a *locally run* `wrangler dev` router.
3. **ADI-MONO-122 — the operator surface.** `/settings/channels` and `adi-mono channels`
   (§7), against whatever ADI-MONO-120/121 shipped. No new protocol here — this task is pure
   plumbing onto the node endpoints already named above.
4. **ADI-MONO-123 — Slack.** The second adapter (§4): OAuth install, per-workspace token storage
   in the DO, signature verification, and Slack's retry/dedup contract. Deliberately after the
   panel/CLI exist, so Slack's install flow (real OAuth, not a deep link) can be built and tested
   against a UI that already has somewhere to show "Connect" for a second provider, instead of
   against raw API calls.
5. **ADI-MONO-124 — go live.** Deploy the router to `hooks.withadi.dev`, register `@AdiBot` with
   Telegram, and whatever Slack's app-directory listing needs. Outward-facing and irreversible in
   ways 1–4 are not (a real bot, a real domain) — **needs Igor's sign-off before any of it runs**,
   per the platform rule against creating real bots/apps or touching DNS without approval.

## Decisions taken

Calls made here that the design brief left open:

1. **Workers, not Pages**, unlike oauth-router — because a Durable Object is the one piece of
   state this design cannot do without (the held-open WebSocket and its offline queue), and Pages
   Functions cannot hold one. Everything else about oauth-router's conventions (registry, signed
   state, env-keyed secrets, small modules) is kept; only the hosting primitive differs, for a
   reason the brief already implied ("Durable Object per node").
2. **A D1 table for routing-key lookup, Durable Object storage for everything else.** A DO cannot
   be queried by anything but its own key, and the webhook path is the one place a request shows
   up holding a routing key and nothing else. Keeping every other fact (connections, allowlists,
   thread maps) inside the owning DO rather than also mirroring it into D1 avoids a second place
   either could drift from the other.
3. **`Marker::From` is reused for channel senders rather than a new marker kind added.** It
   already means "who said this, when more than one voice can reach this conversation," which is
   exactly a Telegram/Slack sender's situation; inventing `Marker::Channel` would say the same
   thing in a second vocabulary for no reader's benefit, and the system prompt that teaches every
   agent about markers needs no change this way.
4. **`channel-reply` is an ordinary `adi-mono tools` script, not an engine-native tool like `Ask`/
   `Report`.** A channel post never blocks the turn the way a question does, so it needs none of
   the harness's suspend/resume machinery — and being an ordinary tool means it works identically
   on every backend (pty, process, harness), not only the ones with a native MCP tool surface.
5. **"Pause" is a flag on an existing connection, not a sixth CLI verb or a new endpoint.** The
   parent task names `connect/list/route/allow/disconnect`; pausing is a property of how the
   target is handled, folded into `route`.
6. **A ninth marketplace element kind, `channel`, is named now and built never (yet).** §6 pins
   the shape so ADI-MONO-120/121's `Connection`/`Target` types don't need revisiting when a bundle
   wants to ship one — the same "design for the future, build only what's asked" split the brief
   draws everywhere else.
