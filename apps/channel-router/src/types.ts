/**
 * The router's runtime configuration and the shapes every module shares. Injected by
 * Cloudflare from `wrangler.toml` `[vars]` / `wrangler secret put` (the scalar fields) and the
 * Durable Object / D1 bindings declared there (the rest). See docs/channels.md §1.
 */
export interface Env {
  /** HMAC key that signs node tokens and link codes (this router's `STATE_SECRET`-equivalent). */
  ROUTER_SECRET: string;
  /** Bearer required by `POST /register`. Gates minting, not ordinary traffic (§8). */
  ROUTER_ADMIN_SECRET: string;

  /** The one Telegram bot ADI runs. */
  TELEGRAM_BOT_TOKEN?: string;
  /** `secret_token` set at `setWebhook` time; echoed back on every delivery. */
  TELEGRAM_SECRET_TOKEN?: string;
  /** Public -- used to build `t.me/<bot>?start=<code>` install links. */
  TELEGRAM_BOT_USERNAME?: string;

  /** One Durable Object per `(node id, provider)` -- see {@link doId}. */
  NODE_CONNECTION: DurableObjectNamespace;
  /** `provider, routing_key -> node_id`. The only thing queried before a node is known (§1). */
  ROUTING_KEYS: D1Database;
}

/** The provider-neutral message schema (v1) -- docs/channels.md §3. */
export interface ChannelMessage {
  v: 1;
  /** Dedup key -- stable across redelivery, unique per (provider, id). */
  id: string;
  provider: string;
  /** The connection id minted at link time -- which agent/trigger/route this is. */
  connection: string;
  /** The provider's own thread/chat identity, opaque past the adapter. */
  thread: string;
  sender: {
    id: string;
    name: string;
  };
  text: string;
  attachments: Array<{
    kind: "image" | "file";
    url: string;
    name?: string;
  }>;
  /** This message's id, if it quoted/replied to an earlier one. */
  reply_to?: string;
  /** What the provider actually called it ("message", "edited_message", "app_mention", ...). */
  raw_kind: string;
  /** Epoch ms, router's clock. */
  received_at: number;
}

/** docs/channels.md §5. Only `agent` is built; `trigger`/`app_route` are named so a bundle-
 * installed channel never has to widen this type later (§6). */
export type Target =
  | { kind: "agent"; agent: string }
  | { kind: "trigger"; trigger: string }
  | { kind: "app_route"; app: string; route: string };

export type Allowlist =
  | { kind: "owner_only" }
  | { kind: "list"; sender_ids: string[] }
  | { kind: "open" };

/** One `(provider, routing key)` bound to one target and one allowlist -- docs/channels.md §5.
 * Lives inside the owning Durable Object; the router never enforces the allowlist itself (§5,
 * §8 -- that's the node's job), it only stores it so the node's mirror has somewhere to come
 * from. */
export interface Connection {
  id: string;
  provider: string;
  /** Null until a link code is consumed against this connection. */
  routing_key: string | null;
  target: Target;
  allowlist: Allowlist;
  /** Provider thread id -> ADI conversation (run) id. The router stores this mirror but never
   * reads or writes it itself -- every route in this task's scope carries the thread id it
   * needs directly in the request, node-side (§5) is the only consumer. */
  threads: Record<string, string>;
  linked: boolean;
}

/** What a node hands the router to post a reply -- the body of `POST /send`, and what an
 * adapter's `send` ultimately delivers. */
export interface OutboundReply {
  /** Chat id / team id -- where to deliver, resolved from the connection by the router. */
  routingKey: string;
  /** Provider thread id, if the service supports native threading. */
  thread?: string;
  text: string;
}

/**
 * One file per service (docs/channels.md §4). `verify`/`routingKey` run before anything else
 * touches a webhook body; `normalize` is async because an adapter may have to round-trip to its
 * own service first (Telegram's photo attachments resolve through `getFile` before forwarding,
 * never after -- see the Telegram section below) -- the spec's original signature had this
 * synchronous, which this build corrects (see docs/channels.md's "Decisions taken").
 *
 * `extractLinkCode` is this build's addition, not in the original spec text: Telegram's "link"
 * is just another webhook (its `/start <code>` arrives as an ordinary `message` update, per §4),
 * so the webhook handler needs a uniform, optional way to ask "is this message actually a link
 * attempt?" without hard-coding Telegram's command syntax into `router.ts`. A provider with a
 * real redirect-based link flow (Slack) simply doesn't implement it.
 */
export interface ChannelAdapter {
  id: string;
  verify(req: Request, env: Env): Promise<boolean>;
  routingKey(payload: unknown): string | null;
  normalize(payload: unknown, connection: string, env: Env): Promise<ChannelMessage[]>;
  send(reply: OutboundReply, credential: unknown): Promise<void>;
  /** Returns the link code if `payload` is a link attempt (e.g. Telegram's `/start <code>`),
   * else `null`. Absent entirely for a provider whose link flow is a real redirect (Slack). */
  extractLinkCode?(payload: unknown): string | null;
}

/** The Durable Object id for one `(node, provider)` pair.
 *
 * docs/channels.md §1 says "one Durable Object per node, keyed by node id", but §2 also says a
 * node talking to both Telegram and Slack "holds two sockets, because each is subscribed
 * through a different Durable Object" -- which only both hold if the key is actually the pair,
 * not the node id alone. This build takes the pair as authoritative (it's also exactly the
 * granularity §8 scopes a node token to) and the spec note above is corrected accordingly.
 */
export function doId(namespace: DurableObjectNamespace, node: string, provider: string): DurableObjectId {
  return namespace.idFromName(`${provider}:${node}`);
}
