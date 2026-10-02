import { describe, it, expect, vi, afterEach, beforeAll, beforeEach } from "vitest";
import { env } from "cloudflare:test";

import { handle } from "../src/router";
import { migrate } from "./migrate";

beforeAll(migrate);

const NOW = 1_700_000_000_000;

interface RegisterResult {
  token: string;
  connection: string;
  link_code: string;
  install_url: string;
}

async function register(
  node: string,
  target: Record<string, unknown> = { kind: "agent", agent: "a" },
  now = NOW,
  provider = "telegram",
): Promise<RegisterResult> {
  const res = await handle(
    new Request("https://router.example/register", {
      method: "POST",
      headers: { authorization: `Bearer ${env.ROUTER_ADMIN_SECRET}` },
      body: JSON.stringify({ node, provider, target }),
    }),
    env,
    now,
  );
  expect(res.status).toBe(200);
  return res.json();
}

async function webhook(body: unknown, now = NOW): Promise<Response> {
  const res = await handle(
    new Request("https://router.example/webhook/telegram", {
      method: "POST",
      headers: { "x-telegram-bot-api-secret-token": env.TELEGRAM_SECRET_TOKEN! },
      body: JSON.stringify(body),
    }),
    env,
    now,
  );
  await res.clone().text(); // drain the body -- see do.ts's note on isolated storage
  return res;
}

async function startUpdate(chatId: number, code: string, updateId = 1): Promise<Response> {
  return webhook({ update_id: updateId, message: { message_id: updateId, chat: { id: chatId }, text: `/start ${code}` } });
}

async function messageUpdate(chatId: number, text: string, updateId: number): Promise<Response> {
  return webhook({
    update_id: updateId,
    message: { message_id: updateId, chat: { id: chatId }, from: { id: 1, username: "alice" }, text },
  });
}

let openSockets: WebSocket[] = [];

async function subscribe(token: string): Promise<WebSocket> {
  const res = await handle(
    new Request("https://router.example/subscribe", {
      headers: { authorization: `Bearer ${token}`, upgrade: "websocket", connection: "Upgrade" },
    }),
    env,
    NOW,
  );
  expect(res.status).toBe(101);
  const ws = res.webSocket as WebSocket;
  ws.accept();
  openSockets.push(ws);
  return ws;
}

function nextFrame(ws: WebSocket): Promise<{ type: string; id?: string; [k: string]: unknown }> {
  return new Promise((resolve) => {
    ws.addEventListener(
      "message",
      (event) => resolve(JSON.parse(typeof event.data === "string" ? event.data : "")),
      { once: true },
    );
  });
}

function sleep(ms: number): Promise<void> {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

let fetchMock: ReturnType<typeof vi.fn>;

beforeEach(() => {
  fetchMock = vi.fn(async () => new Response("{}", { status: 200 }));
  vi.stubGlobal("fetch", fetchMock);
});

afterEach(() => {
  vi.unstubAllGlobals();
  for (const ws of openSockets) ws.close();
  openSockets = [];
});

describe("link-code flow", () => {
  it("links a chat, records the routing key, and sends a welcome message", async () => {
    const node = `node-link-${Math.random()}`;
    const { link_code, connection } = await register(node);

    const res = await startUpdate(4242, link_code, 1);
    expect(res.status).toBe(200);

    // The welcome message went out through the adapter's send -- i.e. through Telegram's own
    // sendMessage, with this deployment's bot token, not anything the test faked directly.
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.telegram.org/bottest-bot-token/sendMessage",
      expect.anything(),
    );

    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "4242")
      .first<{ node_id: string }>();
    expect(row?.node_id).toBe(node);
    void connection;
  });

  it("spends the link code on use -- its D1 row is gone afterwards", async () => {
    const { link_code } = await register(`node-spent-${Math.random()}`);
    const pending = () => env.ROUTING_KEYS.prepare("SELECT 1 FROM link_codes WHERE handle = ?").bind(link_code).first();
    expect(await pending()).not.toBeNull();
    await startUpdate(4244, link_code, 1);
    expect(await pending()).toBeNull();
  });

  it("rejects a forged link code -- the webhook still 200s, but nothing links", async () => {
    fetchMock.mockClear();
    const res = await startUpdate(4243, "forged-code", 1);
    expect(res.status).toBe(200);
    // ...and the chat is told why, rather than met with silence.
    const sent = fetchMock.mock.calls.find(([url]) => String(url).endsWith("/sendMessage"));
    expect(JSON.parse(String((sent?.[1] as RequestInit).body)).text).toMatch(/expired or was already used/);
    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "4243")
      .first();
    expect(row).toBeNull();
  });

  it("refuses to relink an already-linked connection (replay safety)", async () => {
    const node = `node-replay-${Math.random()}`;
    const { link_code } = await register(node);
    await startUpdate(5000, link_code, 1);
    fetchMock.mockClear();

    // Replaying the same code against a *different* chat must not move the binding.
    await startUpdate(6000, link_code, 2);
    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "6000")
      .first();
    expect(row).toBeNull();
  });
});

describe("first install wins (ADI-MONO-125)", () => {
  it("refuses a second node's /start for a chat another node already linked, and says why", async () => {
    const firstNode = `node-steal-first-${Math.random()}`;
    const { link_code: firstCode } = await register(firstNode);
    await startUpdate(20_000, firstCode, 1);

    const secondNode = `node-steal-second-${Math.random()}`;
    const { link_code: secondCode } = await register(secondNode);
    fetchMock.mockClear();
    const res = await startUpdate(20_000, secondCode, 2);
    expect(res.status).toBe(200);

    const sent = fetchMock.mock.calls.find(([url]) => String(url).endsWith("/sendMessage"));
    expect(JSON.parse(String((sent?.[1] as RequestInit).body)).text).toMatch(
      /already connected to another ADI/,
    );
    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "20000")
      .first<{ node_id: string }>();
    expect(row?.node_id).toBe(firstNode);
  });

  it("lets the same node relink a chat it already owns, without a /disconnect first", async () => {
    const node = `node-relink-same-${Math.random()}`;
    const first = await register(node);
    await startUpdate(21_000, first.link_code, 1);

    // A second connection on the *same* node, linked against the *same* chat -- no disconnect
    // in between.
    const second = await register(node);
    const res = await startUpdate(21_000, second.link_code, 2);
    expect(res.status).toBe(200);

    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "21000")
      .first<{ node_id: string }>();
    expect(row?.node_id).toBe(node);

    // The first connection is cleanly released, not left behind as a ghost still claiming
    // `linked: true` for a routing key that no longer reaches it.
    const sendRes = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${first.token}` },
        body: JSON.stringify({ connection: first.connection, text: "hi" }),
      }),
      env,
      NOW,
    );
    expect(sendRes.status).toBe(404); // unlinked, not a steal victim
  });

  it("lets a chat be relinked to a different node after a /disconnect", async () => {
    const firstNode = `node-disconnect-relink-first-${Math.random()}`;
    const first = await register(firstNode);
    await startUpdate(22_000, first.link_code, 1);

    const disconnectRes = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${first.token}` },
        body: JSON.stringify({ connection: first.connection }),
      }),
      env,
      NOW,
    );
    expect(disconnectRes.status).toBe(200);

    const secondNode = `node-disconnect-relink-second-${Math.random()}`;
    const second = await register(secondNode);
    const res = await startUpdate(22_000, second.link_code, 2);
    expect(res.status).toBe(200);

    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "22000")
      .first<{ node_id: string }>();
    expect(row?.node_id).toBe(secondNode);
  });
});

describe("Telegram 'Add to a group' (ADI-MONO-125)", () => {
  it("links from /start@<bot> <code>, the form Telegram sends in a group", async () => {
    const node = `node-group-${Math.random()}`;
    const { link_code } = await register(node);

    const res = await webhook({
      update_id: 1,
      message: { message_id: 1, chat: { id: 23_000 }, text: `/start@TestAdiBot ${link_code}` },
    });
    expect(res.status).toBe(200);

    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "23000")
      .first<{ node_id: string }>();
    expect(row?.node_id).toBe(node);
  });
});

describe("routing", () => {
  it("delivers an ordinary message for a linked chat to the node over its socket", async () => {
    const node = `node-route-${Math.random()}`;
    const { token, link_code } = await register(node);
    await startUpdate(7000, link_code, 1);

    const ws = await subscribe(token);
    const pending = nextFrame(ws); // the "linked" frame, queued at link time
    const linkedFrame = await pending;
    expect(linkedFrame).toMatchObject({ type: "linked", provider: "telegram", routing_key: "7000" });

    const next = nextFrame(ws);
    const res = await messageUpdate(7000, "hello agent", 2);
    expect(res.status).toBe(200);

    const frame = await next;
    expect(frame.type).toBe("event");
    expect((frame.message as { text: string; provider: string }).text).toBe("hello agent");
    expect((frame.message as { provider: string }).provider).toBe("telegram");
  });
});

async function signedSlackRequest(bodyObj: unknown, now = NOW): Promise<Request> {
  const body = JSON.stringify(bodyObj);
  const timestamp = String(Math.floor(now / 1000));
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey(
    "raw",
    enc.encode(env.SLACK_SIGNING_SECRET!),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign"],
  );
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(`v0:${timestamp}:${body}`));
  const hex = Array.from(new Uint8Array(sig))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return new Request("https://router.example/webhook/slack", {
    method: "POST",
    headers: { "x-slack-signature": `v0=${hex}`, "x-slack-request-timestamp": timestamp },
    body,
  });
}

async function slackWebhook(bodyObj: unknown, now = NOW): Promise<Response> {
  const res = await handle(await signedSlackRequest(bodyObj, now), env, now);
  await res.clone().text(); // drain the body -- see do.ts's note on isolated storage
  return res;
}

describe("Slack sender name resolution", () => {
  it("resolves a sender's display name through users.info before delivering the event", async () => {
    const node = `node-slack-sender-${Math.random()}`;
    const { token, link_code } = await register(node, { kind: "agent", agent: "a" }, NOW, "slack");

    fetchMock.mockImplementation(async () =>
      new Response(JSON.stringify({ ok: true, access_token: "xoxb-sender-test", team: { id: "T-SENDER" } }), {
        status: 200,
      }),
    );
    const linkRes = await handle(
      new Request(`https://router.example/link/slack?code=oauth-code&state=${encodeURIComponent(link_code)}`),
      env,
      NOW,
    );
    expect(linkRes.status).toBe(200);

    const ws = await subscribe(token);
    await nextFrame(ws); // the "linked" frame, queued at link time

    fetchMock.mockImplementation(async (url: string) =>
      String(url).startsWith("https://slack.com/api/users.info")
        ? new Response(JSON.stringify({ ok: true, user: { profile: { display_name: "Resolved Name" } } }), {
            status: 200,
          })
        : new Response("{}", { status: 200 }),
    );

    const next = nextFrame(ws);
    const res = await slackWebhook({
      type: "event_callback",
      event_id: "Ev-sender-1",
      team_id: "T-SENDER",
      event: { type: "app_mention", user: "U-RAW", text: "hi", channel: "C1" },
    });
    expect(res.status).toBe(200);

    const frame = await next;
    expect((frame.message as { sender: { id: string; name: string } }).sender).toEqual({
      id: "U-RAW",
      name: "Resolved Name",
    });
  });

  it("falls back to the raw sender id when users.info fails -- never drops the event over it", async () => {
    const node = `node-slack-sender-fail-${Math.random()}`;
    const { token, link_code } = await register(node, { kind: "agent", agent: "a" }, NOW, "slack");

    fetchMock.mockImplementation(async () =>
      new Response(JSON.stringify({ ok: true, access_token: "xoxb-sender-fail", team: { id: "T-SENDER-FAIL" } }), {
        status: 200,
      }),
    );
    await handle(
      new Request(`https://router.example/link/slack?code=oauth-code&state=${encodeURIComponent(link_code)}`),
      env,
      NOW,
    );

    const ws = await subscribe(token);
    await nextFrame(ws); // the "linked" frame

    fetchMock.mockImplementation(async (url: string) =>
      String(url).startsWith("https://slack.com/api/users.info")
        ? new Response(JSON.stringify({ ok: false, error: "user_not_found" }), { status: 200 })
        : new Response("{}", { status: 200 }),
    );

    const next = nextFrame(ws);
    const res = await slackWebhook({
      type: "event_callback",
      event_id: "Ev-sender-2",
      team_id: "T-SENDER-FAIL",
      event: { type: "app_mention", user: "U-RAW-2", text: "hi", channel: "C1" },
    });
    expect(res.status).toBe(200);

    const frame = await next;
    expect((frame.message as { sender: { id: string; name: string } }).sender).toEqual({
      id: "U-RAW-2",
      name: "U-RAW-2",
    });
  });
});

describe("offline queueing and redelivery", () => {
  it("queues events while offline, delivers them on connect, and redelivers an unacked one on reconnect", async () => {
    const node = `node-offline-${Math.random()}`;
    const { token, link_code } = await register(node);
    await startUpdate(8000, link_code, 1);

    // Two messages arrive with nobody subscribed.
    await messageUpdate(8000, "first", 2);
    await messageUpdate(8000, "second", 3);

    const ws1 = await subscribe(token);
    const linked = await nextFrame(ws1); // flushed first, queued at link time
    expect(linked.type).toBe("linked");
    const first = await nextFrame(ws1);
    const second = await nextFrame(ws1);
    expect((first.message as { text: string }).text).toBe("first");
    expect((second.message as { text: string }).text).toBe("second");

    // Ack only the first message, then "disconnect" (simulated by simply not reading from
    // ws1 again -- see do.ts: a fresh /subscribe just replaces the attached socket).
    ws1.send(JSON.stringify({ type: "ack", id: first.id }));
    await sleep(50);

    const ws2 = await subscribe(token);
    const redelivered = await nextFrame(ws2);
    // The acked "first" is gone; the unacked "second" comes back on reconnect.
    expect((redelivered.message as { text: string }).text).toBe("second");
  });
});

describe("POST /disconnect", () => {
  it("drops the connection and the D1 routing row, and revokes the token once it was the last connection", async () => {
    const node = `node-disconnect-ok-${Math.random()}`;
    const { token, connection, link_code } = await register(node);
    await startUpdate(10_000, link_code, 1);

    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "10000")
      .first();
    expect(row).not.toBeNull();

    const res = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ connection }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true, revoked: true });

    const rowAfter = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("telegram", "10000")
      .first();
    expect(rowAfter).toBeNull();

    // The connection is gone -- a /send against it 404s, same as "never linked".
    const sendRes = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ connection, text: "hi" }),
      }),
      env,
      NOW,
    );
    expect(sendRes.status).toBe(401); // the token itself was revoked, caught before the connection lookup
  });

  it("keeps the token alive when another connection on the same (node, provider) still uses it", async () => {
    const node = `node-disconnect-shared-${Math.random()}`;
    const first = await register(node);
    const second = await register(node);
    expect(second.token).toBe(first.token);

    const res = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${first.token}` },
        body: JSON.stringify({ connection: first.connection }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true, revoked: false });

    // The token still works, e.g. against the surviving connection.
    const checkRes = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${first.token}` },
        body: JSON.stringify({ connection: second.connection, text: "hi" }),
      }),
      env,
      NOW,
    );
    expect(checkRes.status).toBe(404); // unlinked, not 401 -- the token itself is still valid
  });

  it("a revoked token can't act on a connection registered after it", async () => {
    const node = `node-disconnect-revoked-${Math.random()}`;
    const first = await register(node);
    await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${first.token}` },
        body: JSON.stringify({ connection: first.connection }),
      }),
      env,
      NOW,
    );

    // A fresh token was minted once the old one was revoked -- a later `now` so its signed
    // payload (node/provider/epoch/t) actually differs from the revoked one's, not just its
    // storage state (signing is a pure function of the payload, so an identical payload at the
    // same instant would otherwise re-mint byte-for-byte the same token).
    const second = await register(node, { kind: "agent", agent: "a" }, NOW + 5000);
    expect(second.token).not.toBe(first.token);

    const res = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${first.token}` },
        body: JSON.stringify({ connection: second.connection }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });
});

describe("POST /send", () => {
  it("posts the reply to telegram once the connection is linked", async () => {
    const node = `node-send-ok-${Math.random()}`;
    const { token, connection, link_code } = await register(node);
    await startUpdate(9000, link_code, 1);
    fetchMock.mockClear();

    const res = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ connection, text: "reply from the agent" }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    await res.json();
    expect(fetchMock).toHaveBeenCalledWith(
      "https://api.telegram.org/bottest-bot-token/sendMessage",
      expect.objectContaining({
        body: JSON.stringify({ chat_id: "9000", text: "reply from the agent" }),
      }),
    );
  });

  it("posts to slack with the per-workspace bot token decrypted out of the connection", async () => {
    const node = `node-slack-send-${Math.random()}`;
    const registerRes = await handle(
      new Request("https://router.example/register", {
        method: "POST",
        headers: { authorization: `Bearer ${env.ROUTER_ADMIN_SECRET}` },
        body: JSON.stringify({ node, provider: "slack", target: { kind: "agent", agent: "a" } }),
      }),
      env,
      NOW,
    );
    const { token, connection, link_code } = (await registerRes.json()) as {
      token: string;
      connection: string;
      link_code: string;
    };

    // A team id unique to this test -- isolatedStorage is off for this whole suite (see
    // vitest.config.ts), so a hardcoded one shared with another test file's own Slack link would
    // collide on the D1 routing_keys row (and, since ADI-MONO-125, get refused as a steal).
    const teamId = `T-SEND-${Math.random()}`;
    fetchMock.mockImplementation(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true, access_token: "xoxb-workspace-1", team: { id: teamId } }), {
        status: 200,
      }),
    );
    const linkRes = await handle(
      new Request(`https://router.example/link/slack?code=oauth-code&state=${encodeURIComponent(link_code)}`),
      env,
      NOW,
    );
    expect(linkRes.status).toBe(200);

    fetchMock.mockClear();
    fetchMock.mockImplementation(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true }), { status: 200 }),
    );
    const sendRes = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ connection, text: "reply from the agent", thread: "C999" }),
      }),
      env,
      NOW,
    );
    expect(sendRes.status).toBe(200);
    expect(fetchMock).toHaveBeenCalledWith(
      "https://slack.com/api/chat.postMessage",
      expect.objectContaining({
        headers: expect.objectContaining({ authorization: "Bearer xoxb-workspace-1" }),
        body: JSON.stringify({ channel: "C999", text: "reply from the agent" }),
      }),
    );
  });
});
