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

async function register(node: string, target: Record<string, unknown> = { kind: "agent", agent: "a" }): Promise<RegisterResult> {
  const res = await handle(
    new Request("https://router.example/register", {
      method: "POST",
      headers: { authorization: `Bearer ${env.ROUTER_ADMIN_SECRET}` },
      body: JSON.stringify({ node, provider: "telegram", target }),
    }),
    env,
    NOW,
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

  it("rejects a forged link code -- the webhook still 200s, but nothing links", async () => {
    const res = await startUpdate(4243, "forged-code", 1);
    expect(res.status).toBe(200);
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
});
