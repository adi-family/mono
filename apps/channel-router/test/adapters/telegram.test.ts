import { describe, it, expect, vi, afterEach } from "vitest";

import { telegramAdapter, telegramInstallUrl } from "../../src/adapters/telegram";
import type { Env } from "../../src/types";

function env(overrides: Partial<Env> = {}): Env {
  return {
    ROUTER_SECRET: "s",
    ROUTER_ADMIN_SECRET: "a",
    TELEGRAM_BOT_TOKEN: "bot-token",
    TELEGRAM_SECRET_TOKEN: "webhook-secret",
    TELEGRAM_BOT_USERNAME: "AdiBot",
    NODE_CONNECTION: undefined as never,
    REGISTER_LIMITER: undefined as never,
    ROUTING_KEYS: undefined as never,
    ...overrides,
  };
}

function update(message: unknown, updateId = 1): unknown {
  return { update_id: updateId, message };
}

afterEach(() => {
  vi.unstubAllGlobals();
});

const NOW = 1_700_000_000;

describe("telegram verify", () => {
  it("accepts the configured secret token header", async () => {
    const req = new Request("https://router.example/webhook/telegram", {
      headers: { "x-telegram-bot-api-secret-token": "webhook-secret" },
    });
    expect(await telegramAdapter.verify(req, env(), NOW)).toBe(true);
  });

  it("rejects a missing header", async () => {
    const req = new Request("https://router.example/webhook/telegram");
    expect(await telegramAdapter.verify(req, env(), NOW)).toBe(false);
  });

  it("rejects a wrong header", async () => {
    const req = new Request("https://router.example/webhook/telegram", {
      headers: { "x-telegram-bot-api-secret-token": "nope" },
    });
    expect(await telegramAdapter.verify(req, env(), NOW)).toBe(false);
  });

  it("rejects when the deployment has no secret configured", async () => {
    const req = new Request("https://router.example/webhook/telegram", {
      headers: { "x-telegram-bot-api-secret-token": "webhook-secret" },
    });
    expect(await telegramAdapter.verify(req, env({ TELEGRAM_SECRET_TOKEN: undefined }), NOW)).toBe(false);
  });
});

describe("telegram routingKey", () => {
  it("is the chat id, as a string", () => {
    const payload = update({ message_id: 1, chat: { id: 555 }, text: "hi" });
    expect(telegramAdapter.routingKey(payload)).toBe("555");
  });

  it("is null for an update with no message", () => {
    expect(telegramAdapter.routingKey({ update_id: 1 })).toBeNull();
  });
});

describe("telegram extractLinkCode", () => {
  it("extracts the code from /start <code>", () => {
    const payload = update({ message_id: 1, chat: { id: 555 }, text: "/start abc123" });
    expect(telegramAdapter.extractLinkCode!(payload)).toBe("abc123");
  });

  it("returns an empty string for a bare /start", () => {
    const payload = update({ message_id: 1, chat: { id: 555 }, text: "/start" });
    expect(telegramAdapter.extractLinkCode!(payload)).toBe("");
  });

  it("returns null for an ordinary message", () => {
    const payload = update({ message_id: 1, chat: { id: 555 }, text: "hello there" });
    expect(telegramAdapter.extractLinkCode!(payload)).toBeNull();
  });
});

describe("telegram normalize", () => {
  it("produces one ChannelMessage per update", async () => {
    const payload = update(
      { message_id: 1, chat: { id: 555 }, from: { id: 42, username: "alice" }, text: "hello" },
      99,
    );
    const messages = await telegramAdapter.normalize(payload, "conn-1", env());
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      v: 1,
      id: "telegram:99",
      provider: "telegram",
      connection: "conn-1",
      thread: "555",
      sender: { id: "42", name: "alice" },
      text: "hello",
      raw_kind: "message",
    });
  });

  it("never normalizes a /start into an ordinary message", async () => {
    const payload = update({ message_id: 1, chat: { id: 555 }, text: "/start code" });
    expect(await telegramAdapter.normalize(payload, "conn-1", env())).toEqual([]);
  });

  it("resolves a photo's largest size through getFile before forwarding", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true, result: { file_path: "photos/big.jpg" } }), {
        headers: { "content-type": "application/json" },
      }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const payload = update({
      message_id: 1,
      chat: { id: 555 },
      photo: [
        { file_id: "small", width: 10, height: 10 },
        { file_id: "big", width: 800, height: 600 },
      ],
    });
    const messages = await telegramAdapter.normalize(payload, "conn-1", env());
    expect(messages[0].attachments).toEqual([
      { kind: "image", url: "https://api.telegram.org/file/botbot-token/photos/big.jpg" },
    ]);
    expect(fetchMock).toHaveBeenCalledWith(
      expect.stringContaining("getFile?file_id=big"),
    );
  });

  it("gives a forum topic its own thread id", async () => {
    const payload = update({
      message_id: 1,
      chat: { id: 555 },
      message_thread_id: 7,
      text: "in a topic",
    });
    const messages = await telegramAdapter.normalize(payload, "conn-1", env());
    expect(messages[0].thread).toBe("555:7");
  });
});

describe("telegram send", () => {
  it("posts to sendMessage with the bot token and chat id", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => new Response("{}", { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await telegramAdapter.send({ routingKey: "555", text: "hi there" }, { botToken: "bot-token" });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl, init] = fetchMock.mock.calls[0];
    expect(calledUrl).toBe("https://api.telegram.org/botbot-token/sendMessage");
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({ chat_id: "555", text: "hi there" });
  });

  it("throws when telegram rejects the send", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("bad", { status: 400 })));
    await expect(
      telegramAdapter.send({ routingKey: "555", text: "hi" }, { botToken: "bot-token" }),
    ).rejects.toThrow();
  });
});

describe("telegram setThinking", () => {
  it("sends a typing chat action for a bare chat thread", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => new Response("{}", { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await telegramAdapter.setThinking!("555", { botToken: "bot-token" }, "thinking…");

    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl, init] = fetchMock.mock.calls[0];
    expect(calledUrl).toBe("https://api.telegram.org/botbot-token/sendChatAction");
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({ chat_id: "555", action: "typing" });
  });

  it("includes message_thread_id for a forum topic's thread key", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) => new Response("{}", { status: 200 }));
    vi.stubGlobal("fetch", fetchMock);

    await telegramAdapter.setThinking!("555:7", { botToken: "bot-token" }, "thinking…");

    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      chat_id: "555",
      action: "typing",
      message_thread_id: 7,
    });
  });

  it("is a no-op when clearing -- there's no cancel call to make", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    await telegramAdapter.setThinking!("555", { botToken: "bot-token" }, "");
    expect(fetchMock).not.toHaveBeenCalled();
  });

  it("throws when telegram rejects the chat action", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => new Response("bad", { status: 400 })));
    await expect(
      telegramAdapter.setThinking!("555", { botToken: "bot-token" }, "thinking…"),
    ).rejects.toThrow();
  });
});

describe("telegramInstallUrl", () => {
  it("builds the t.me deep link", () => {
    expect(telegramInstallUrl(env(), "abc123")).toBe("https://t.me/AdiBot?start=abc123");
  });

  it("is null without a configured bot username", () => {
    expect(telegramInstallUrl(env({ TELEGRAM_BOT_USERNAME: undefined }), "abc123")).toBeNull();
  });
});
