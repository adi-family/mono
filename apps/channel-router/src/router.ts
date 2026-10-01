/**
 * The channel router. One deployment fronts many providers; every webhook and every node
 * socket passes through here on the way to or from a `NodeConnection` Durable Object
 * (`do.ts`). docs/channels.md §1 is the route table this mirrors.
 *
 * - `POST /webhook/<provider>` -- verify the service's signature, resolve the routing key
 *   (chat id / team id) to a node via the `routing_keys` D1 table, forward to that node's DO.
 *   A `/start <code>` is a link attempt, not an ordinary message (§4's `extractLinkCode`), and
 *   is handled before `normalize` ever sees it.
 * - `GET /link/<provider>` -- a real redirect-based install flow (Slack's "Add to Slack").
 *   Nobody needs it yet: Telegram links through its webhook instead.
 * - `POST /register` -- admin-gated, mints a node token (first time only) and a fresh
 *   connection + link code.
 * - `POST /subscribe` -- a node's WebSocket upgrade, forwarded to its DO once the bearer node
 *   token checks out.
 * - `POST /send` -- a node's outbound reply, authenticated the same way, resolved to a chat id
 *   through the connection's DO record and posted with the provider's own credential.
 */
import { getAdapter, resolveCredential } from "./adapters/index";
import { telegramInstallUrl } from "./adapters/telegram";
import { bearerToken, json, problem, timingSafeEqual } from "./http";
import { enabledProviders } from "./providers";
import { verifyLinkCode, verifyNodeToken } from "./state";
import { doId } from "./types";
import type { ChannelAdapter, Connection, Env, OutboundReply, Target } from "./types";

/**
 * Route a request. `now` (epoch ms) is injected so tests can pin the clock, matching
 * oauth-router's `handle` convention.
 */
export async function handle(request: Request, env: Env, now: number = Date.now()): Promise<Response> {
  const url = new URL(request.url);
  const segments = url.pathname.split("/").filter(Boolean);

  if (request.method === "GET" && (segments.length === 0 || segments[0] === "health")) {
    return json({ service: "adi-channel-router", providers: enabledProviders(env) });
  }

  if (request.method === "POST" && segments.length === 2 && segments[0] === "webhook") {
    return handleWebhook(segments[1], request, env, now);
  }

  if (request.method === "GET" && segments.length === 2 && segments[0] === "link") {
    return handleLink(segments[1]);
  }

  if (request.method === "POST" && segments.length === 1 && segments[0] === "register") {
    return handleRegister(request, env, now);
  }

  if (segments.length === 1 && segments[0] === "subscribe") {
    return handleSubscribe(request, env, now);
  }

  if (request.method === "POST" && segments.length === 1 && segments[0] === "send") {
    return handleSend(request, env, now);
  }

  return problem(404, "not found");
}

interface RegisterBody {
  node?: unknown;
  provider?: unknown;
  target?: Target;
  allowlist?: unknown;
}

/** `POST /register` -- mint a node token (first call for this `(node, provider)` only) and a
 * fresh pending connection + link code, behind `ROUTER_ADMIN_SECRET` (§1 step 1, §8). */
async function handleRegister(request: Request, env: Env, now: number): Promise<Response> {
  const admin = bearerToken(request);
  if (!admin || !timingSafeEqual(admin, env.ROUTER_ADMIN_SECRET)) {
    return problem(401, "missing or invalid admin bearer");
  }

  let body: RegisterBody;
  try {
    body = await request.json();
  } catch {
    return problem(400, "expected a JSON body");
  }
  if (typeof body.node !== "string" || !body.node) return problem(400, "missing node");
  if (typeof body.provider !== "string" || !body.provider) return problem(400, "missing provider");
  if (!getAdapter(body.provider)) return problem(404, `unknown or unsupported provider: ${body.provider}`);
  if (!body.target) return problem(400, "missing target");

  const stub = env.NODE_CONNECTION.get(doId(env.NODE_CONNECTION, body.node, body.provider));
  const res = await stub.fetch("https://do/internal/register", {
    method: "POST",
    body: JSON.stringify({
      node: body.node,
      provider: body.provider,
      target: body.target,
      allowlist: body.allowlist,
      now: Math.floor(now / 1000),
    }),
  });
  const data = (await res.json()) as { token: string; connection: string; link_code: string };

  const installUrl = body.provider === "telegram" ? telegramInstallUrl(env, data.link_code) : null;
  return json({ token: data.token, connection: data.connection, link_code: data.link_code, install_url: installUrl });
}

/** `POST /webhook/<provider>` -- §1's webhook row, §4's `verify`/`routingKey`/`normalize`. */
async function handleWebhook(providerId: string, request: Request, env: Env, now: number): Promise<Response> {
  const adapter = getAdapter(providerId);
  if (!adapter) return problem(404, `unknown or unsupported provider: ${providerId}`);

  const verified = await adapter.verify(request, env);
  if (!verified) return problem(400, "signature verification failed");

  let payload: unknown;
  try {
    payload = await request.json();
  } catch {
    return problem(400, "expected a JSON body");
  }

  const routingKey = adapter.routingKey(payload);
  if (routingKey === null) {
    // No chat/team id on this payload at all (e.g. a provider's own health-check ping) --
    // nothing to route, and nothing wrong either.
    return json({ ok: true });
  }

  const linkCode = adapter.extractLinkCode?.(payload) ?? null;
  if (linkCode !== null) {
    return handleLinkAttempt(adapter, providerId, linkCode, routingKey, env, now);
  }

  const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
    .bind(providerId, routingKey)
    .first<{ node_id: string }>();
  if (!row) {
    // An unlinked chat -- no node to reach, so stay silent rather than telling a stranger
    // a bot exists here (same default §5/§8 draw for the allowlist).
    return json({ ok: true });
  }

  const stub = env.NODE_CONNECTION.get(doId(env.NODE_CONNECTION, row.node_id, providerId));
  const lookupRes = await stub.fetch("https://do/internal/connection/lookup", {
    method: "POST",
    body: JSON.stringify({ routing_key: routingKey }),
  });
  const { connection } = (await lookupRes.json()) as { connection: string | null };
  if (!connection) return json({ ok: true });

  const messages = await adapter.normalize(payload, connection, env);
  for (const message of messages) {
    const deliverRes = await stub.fetch("https://do/internal/deliver", {
      method: "POST",
      body: JSON.stringify({ message }),
    });
    await deliverRes.json(); // every response body must be drained, even unused -- see do.ts
  }

  // The webhook is accepted as soon as the event is queued -- delivery to the node over its
  // socket isn't on this request's critical path (§1: a slow 200 earns more retries than a
  // fast one, not fewer).
  return json({ ok: true });
}

/** A `/start <code>` (or, later, a real `GET /link/<provider>` callback) consuming a signed
 * link code against the pending connection it names (§1 step 3). */
async function handleLinkAttempt(
  adapter: ChannelAdapter,
  providerId: string,
  code: string,
  routingKey: string,
  env: Env,
  now: number,
): Promise<Response> {
  if (!code) return json({ ok: true }); // a bare /start with no code -- nothing to link yet

  const payload = await verifyLinkCode(code, env.ROUTER_SECRET, Math.floor(now / 1000));
  if (!payload || payload.provider !== providerId) {
    // Invalid or expired code: still ack the webhook so the service doesn't retry it, the
    // human just has to run `connect` again for a fresh one.
    return json({ ok: true });
  }

  const stub = env.NODE_CONNECTION.get(doId(env.NODE_CONNECTION, payload.node, providerId));
  const linkRes = await stub.fetch("https://do/internal/connection/link", {
    method: "POST",
    body: JSON.stringify({ connection: payload.connection, routing_key: routingKey }),
  });
  const linkOk = linkRes.ok;
  await linkRes.json(); // every response body must be drained, even unused -- see do.ts
  if (!linkOk) return json({ ok: true }); // unknown or already-linked connection -- no-op

  await env.ROUTING_KEYS.prepare("INSERT OR REPLACE INTO routing_keys (provider, routing_key, node_id) VALUES (?, ?, ?)")
    .bind(providerId, routingKey, payload.node)
    .run();

  const credential = resolveCredential(providerId, env);
  if (credential) {
    try {
      await adapter.send({ routingKey, text: "Linked. You can talk to the agent here now." }, credential);
    } catch {
      // The link itself succeeded; a failed welcome message isn't worth failing the webhook.
    }
  }
  return json({ ok: true });
}

/** `GET /link/<provider>` -- a real redirect-based install hop. Telegram links through its
 * webhook instead (`extractLinkCode`), so this 404s for every provider built so far; it exists
 * for Slack's "Add to Slack" OAuth callback (§4), not built in this task. */
function handleLink(providerId: string): Response {
  const adapter = getAdapter(providerId);
  if (!adapter) return problem(404, `unknown or unsupported provider: ${providerId}`);
  return problem(404, `${providerId} has no redirect-based link flow`);
}

/** `POST /subscribe` -- verify the node token, then forward the upgrade itself to the owning
 * DO so it can accept the socket (§1, §2, §8). */
async function handleSubscribe(request: Request, env: Env, now: number): Promise<Response> {
  const url = new URL(request.url);
  const token = bearerToken(request) ?? url.searchParams.get("token");
  if (!token) return problem(401, "missing node token");

  const payload = await verifyNodeToken(token, env.ROUTER_SECRET, Math.floor(now / 1000));
  if (!payload) return problem(401, "invalid or expired node token");

  const stub = env.NODE_CONNECTION.get(doId(env.NODE_CONNECTION, payload.node, payload.provider));
  const forwardUrl = new URL("https://do/internal/subscribe");
  forwardUrl.searchParams.set("epoch", String(payload.epoch));
  return stub.fetch(new Request(forwardUrl.toString(), request));
}

interface SendBody {
  connection?: unknown;
  text?: unknown;
  thread?: unknown;
}

/** `POST /send` -- the only path a reply takes (§1): resolve the connection's chat/team id
 * from its owning DO, then call the provider with this deployment's own credential. */
async function handleSend(request: Request, env: Env, now: number): Promise<Response> {
  const token = bearerToken(request);
  if (!token) return problem(401, "missing node token");

  const payload = await verifyNodeToken(token, env.ROUTER_SECRET, Math.floor(now / 1000));
  if (!payload) return problem(401, "invalid or expired node token");

  let body: SendBody;
  try {
    body = await request.json();
  } catch {
    return problem(400, "expected a JSON body");
  }
  if (typeof body.connection !== "string" || typeof body.text !== "string") {
    return problem(400, "expected { connection, text }");
  }

  const stub = env.NODE_CONNECTION.get(doId(env.NODE_CONNECTION, payload.node, payload.provider));

  const checkRes = await stub.fetch("https://do/internal/token/check", {
    method: "POST",
    body: JSON.stringify({ epoch: payload.epoch }),
  });
  const { valid } = (await checkRes.json()) as { valid: boolean };
  if (!valid) return problem(401, "node token no longer valid");

  const getRes = await stub.fetch("https://do/internal/connection/get", {
    method: "POST",
    body: JSON.stringify({ connection: body.connection }),
  });
  const { connection } = (await getRes.json()) as { connection: Connection | null };
  if (!connection || !connection.linked || !connection.routing_key) {
    return problem(404, "unknown or unlinked connection");
  }

  const adapter = getAdapter(payload.provider);
  const credential = adapter ? resolveCredential(payload.provider, env) : null;
  if (!adapter || !credential) {
    return problem(502, `provider ${payload.provider} is not configured on this deployment`);
  }

  const reply: OutboundReply = {
    routingKey: connection.routing_key,
    text: body.text,
    thread: typeof body.thread === "string" ? body.thread : undefined,
  };
  try {
    await adapter.send(reply, credential);
  } catch (e) {
    return problem(502, `send failed: ${String(e)}`);
  }
  return json({ ok: true });
}
