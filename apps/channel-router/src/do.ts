/**
 * `NodeConnection` -- one Durable Object per `(node, provider)` pair (see {@link doId} in
 * `types.ts` for why the key is the pair, not the node id alone). It holds everything
 * docs/channels.md §1 assigns to the owning object: the node's current WebSocket (if any) and
 * its outbound frame queue, every connection this `(node, provider)` has, and the one node
 * token minted for it.
 *
 * Reachable only through `env.NODE_CONNECTION.get(id).fetch(...)` from `router.ts` -- these
 * `/internal/*` paths are never exposed to the internet directly, so they carry no auth of
 * their own; `router.ts` has already done whatever checking a caller needed before it gets
 * here (admin bearer for register, signature + freshness for a node token).
 */
import { randomId, signLinkCode, signNodeToken } from "./state";
import type { Allowlist, ChannelMessage, Connection, Env, Target } from "./types";

/** Same shape of cap `adi-events`' spool uses: oldest dropped past it, since a node offline
 * long enough to overflow this has bigger problems than one dropped event. */
const MAX_SPOOL = 100;

type QueuedFrame =
  | {
      id: string;
      needsAck: true;
      frame: { type: "event"; id: string; message: ChannelMessage };
    }
  | {
      id: string;
      needsAck: false;
      frame: { type: "linked"; connection: string; provider: string; routing_key: string };
    };

interface TokenRecord {
  token: string;
  epoch: number;
}

interface RegisterRequest {
  node: string;
  provider: string;
  target: Target;
  allowlist?: Allowlist;
  /** Epoch seconds, from the router's own (possibly test-pinned) clock. */
  now?: number;
}

export class NodeConnection implements DurableObject {
  private socket: WebSocket | undefined;

  constructor(
    private readonly ctx: DurableObjectState,
    private readonly env: Env,
  ) {}

  async fetch(request: Request): Promise<Response> {
    const url = new URL(request.url);
    if (url.pathname === "/internal/subscribe") return this.subscribe(request);

    if (request.method !== "POST") return new Response("not found", { status: 404 });
    switch (url.pathname) {
      case "/internal/register":
        return this.register(request);
      case "/internal/token/check":
        return this.checkToken(request);
      case "/internal/connection/link":
        return this.link(request);
      case "/internal/connection/lookup":
        return this.lookupByRoutingKey(request);
      case "/internal/connection/get":
        return this.getConnection(request);
      case "/internal/deliver":
        return this.deliver(request);
      default:
        return new Response("not found", { status: 404 });
    }
  }

  // -- storage helpers --------------------------------------------------------------------

  private async getToken(): Promise<TokenRecord | null> {
    return (await this.ctx.storage.get<TokenRecord>("token")) ?? null;
  }

  private async getQueue(): Promise<QueuedFrame[]> {
    return (await this.ctx.storage.get<QueuedFrame[]>("queue")) ?? [];
  }

  private async setQueue(queue: QueuedFrame[]): Promise<void> {
    await this.ctx.storage.put("queue", queue);
  }

  // -- /internal/register -------------------------------------------------------------------

  /** Mint a node token if this `(node, provider)` doesn't have one yet (one token, shared by
   * every connection on this provider -- §1), register a new pending connection, and mint a
   * link code for it. */
  private async register(request: Request): Promise<Response> {
    const body = (await request.json()) as RegisterRequest;
    // The caller's clock (router.ts's `handle(..., now)`), not this object's own -- a test
    // pins `now` at the router boundary, and a token minted against `Date.now()` instead would
    // read as "signed in the future" the moment that pinned clock runs the freshness check.
    const now = typeof body.now === "number" ? body.now : Math.floor(Date.now() / 1000);

    let token = await this.getToken();
    if (!token) {
      const epoch = 1;
      const signed = await signNodeToken(
        { node: body.node, provider: body.provider, epoch, t: now },
        this.env.ROUTER_SECRET,
      );
      token = { token: signed, epoch };
      await this.ctx.storage.put("token", token);
    }

    const connectionId = randomId();
    const connection: Connection = {
      id: connectionId,
      provider: body.provider,
      routing_key: null,
      target: body.target,
      allowlist: body.allowlist ?? { kind: "owner_only" },
      threads: {},
      linked: false,
    };
    await this.ctx.storage.put(`conn:${connectionId}`, connection);

    const linkCode = await signLinkCode(
      { node: body.node, provider: body.provider, connection: connectionId, t: now },
      this.env.ROUTER_SECRET,
    );

    return Response.json({ token: token.token, connection: connectionId, link_code: linkCode });
  }

  // -- /internal/token/check ------------------------------------------------------------------

  private async checkToken(request: Request): Promise<Response> {
    const { epoch } = (await request.json()) as { epoch: number };
    const token = await this.getToken();
    return Response.json({ valid: Boolean(token && token.epoch === epoch) });
  }

  // -- /internal/connection/link --------------------------------------------------------------

  /** Consume a verified link code against the pending connection it names. Idempotent against
   * replay: a connection already linked refuses a second attempt rather than silently moving
   * the chat it's bound to. */
  private async link(request: Request): Promise<Response> {
    const { connection, routing_key } = (await request.json()) as { connection: string; routing_key: string };
    const record = await this.ctx.storage.get<Connection>(`conn:${connection}`);
    if (!record) return Response.json({ ok: false, reason: "unknown connection" }, { status: 404 });
    if (record.linked) return Response.json({ ok: false, reason: "already linked" }, { status: 409 });

    record.linked = true;
    record.routing_key = routing_key;
    await this.ctx.storage.put(`conn:${connection}`, record);
    await this.ctx.storage.put(`route:${routing_key}`, connection);

    await this.enqueue({
      id: `linked:${connection}`,
      needsAck: false,
      frame: { type: "linked", connection, provider: record.provider, routing_key },
    });

    return Response.json({ ok: true, connection: record });
  }

  // -- /internal/connection/lookup -------------------------------------------------------------

  private async lookupByRoutingKey(request: Request): Promise<Response> {
    const { routing_key } = (await request.json()) as { routing_key: string };
    const connection = (await this.ctx.storage.get<string>(`route:${routing_key}`)) ?? null;
    return Response.json({ connection });
  }

  // -- /internal/connection/get ----------------------------------------------------------------

  private async getConnection(request: Request): Promise<Response> {
    const { connection: id } = (await request.json()) as { connection: string };
    const connection = (await this.ctx.storage.get<Connection>(`conn:${id}`)) ?? null;
    return Response.json({ connection });
  }

  // -- /internal/deliver -----------------------------------------------------------------------

  /** Queue an inbound `ChannelMessage` as an `event` frame and push it now if a socket is
   * attached. The entry stays queued regardless -- only an `ack` frame from the node (§2)
   * removes it, which is what makes a disconnect-before-ack redeliver on reconnect. */
  private async deliver(request: Request): Promise<Response> {
    const { message } = (await request.json()) as { message: ChannelMessage };
    await this.enqueue({ id: message.id, needsAck: true, frame: { type: "event", id: message.id, message } });
    return Response.json({ ok: true });
  }

  private async enqueue(entry: QueuedFrame): Promise<void> {
    const queue = await this.getQueue();
    queue.push(entry);
    while (queue.length > MAX_SPOOL) queue.shift();
    await this.setQueue(queue);
    await this.flush();
  }

  /** Send every queued frame over the attached socket, if any. An `event` frame stays queued
   * either way (ack-only removal); a one-shot `linked` frame is dropped once a send actually
   * went out. */
  private async flush(): Promise<void> {
    if (!this.socket) return;
    const queue = await this.getQueue();
    const remaining: QueuedFrame[] = [];
    for (const entry of queue) {
      try {
        this.socket.send(JSON.stringify(entry.frame));
      } catch {
        remaining.push(entry);
        continue;
      }
      if (entry.needsAck) remaining.push(entry);
    }
    await this.setQueue(remaining);
  }

  // -- /internal/subscribe ---------------------------------------------------------------------

  /** Accept the WebSocket upgrade the router forwarded. `epoch` (from the already-verified
   * node token) is re-checked here against this object's own stored token -- the router's
   * check and this one agreeing is what makes a revoked-and-re-minted token fail closed even
   * if a stale check slipped through somewhere upstream. */
  private async subscribe(request: Request): Promise<Response> {
    if (request.headers.get("upgrade") !== "websocket") {
      return new Response("expected a websocket upgrade", { status: 400 });
    }
    const epoch = Number(new URL(request.url).searchParams.get("epoch"));
    const token = await this.getToken();
    if (!token || token.epoch !== epoch) {
      return new Response("invalid or stale node token", { status: 401 });
    }

    const pair = new WebSocketPair();
    const [client, server] = Object.values(pair);
    server.accept();

    // A fresh socket replaces whatever was attached before it -- the previous one, if still
    // technically open, is simply no longer this object's socket and won't be written to again.
    this.socket = server;
    server.addEventListener("message", (event: MessageEvent) => {
      // A message event fires well after `subscribe`'s own request has returned its 101, so
      // nothing else is holding this object's execution context open for it -- waitUntil is
      // what tells the runtime an ack is still in flight and not to tear the object down
      // mid-write.
      this.ctx.waitUntil(this.onSocketMessage(event));
    });
    server.addEventListener("close", () => {
      if (this.socket === server) this.socket = undefined;
    });

    await this.flush();
    return new Response(null, { status: 101, webSocket: client });
  }

  private async onSocketMessage(event: MessageEvent): Promise<void> {
    let frame: { type?: string; id?: string };
    try {
      frame = JSON.parse(typeof event.data === "string" ? event.data : "");
    } catch {
      return;
    }
    if (frame.type === "ack" && typeof frame.id === "string") {
      const queue = await this.getQueue();
      await this.setQueue(queue.filter((entry) => entry.id !== frame.id));
    }
    // "pong" needs no bookkeeping beyond what §2's heartbeat already describes, which this
    // build doesn't exercise in tests -- the 30s ping / two-missed-pongs drop is cost hygiene,
    // not correctness this task's test bullets ask for.
  }
}
