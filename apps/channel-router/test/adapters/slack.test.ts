import { describe, it, expect, vi, afterEach } from "vitest";

import { slackAdapter, slackExchangeCode, slackInstallUrl, slackRedirectUri } from "../../src/adapters/slack";
import type { Env } from "../../src/types";

const SECRET = "slack-signing-secret";
const NOW = 1_700_000_000; // epoch seconds -- ChannelAdapter.verify's `now` is seconds, not ms

function env(overrides: Partial<Env> = {}): Env {
  return {
    ROUTER_SECRET: "s",
    ROUTER_ADMIN_SECRET: "a",
    SLACK_CLIENT_ID: "client-id",
    SLACK_CLIENT_SECRET: "client-secret",
    SLACK_SIGNING_SECRET: SECRET,
    NODE_CONNECTION: undefined as never,
    ROUTING_KEYS: undefined as never,
    ...overrides,
  };
}

/** A correctly-signed webhook request, Slack's own documented scheme: HMAC-SHA256 of
 * `v0:<timestamp>:<raw body>` under the signing secret, hex, prefixed `v0=`. */
async function signedRequest(body: string, timestamp = String(NOW), secret = SECRET): Promise<Request> {
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
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

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("slack verify", () => {
  it("accepts a correctly-signed request within the freshness window", async () => {
    const req = await signedRequest('{"type":"url_verification"}');
    expect(await slackAdapter.verify(req, env(), NOW)).toBe(true);
  });

  it("rejects a tampered body -- the signature no longer matches", async () => {
    const req = await signedRequest('{"type":"url_verification"}');
    const tampered = new Request(req.url, { method: "POST", headers: req.headers, body: '{"type":"evil"}' });
    expect(await slackAdapter.verify(tampered, env(), NOW)).toBe(false);
  });

  it("rejects a missing signature or timestamp header", async () => {
    const req = new Request("https://router.example/webhook/slack", { method: "POST", body: "{}" });
    expect(await slackAdapter.verify(req, env(), NOW)).toBe(false);
  });

  it("rejects a timestamp outside the five-minute window", async () => {
    const stale = String(NOW - 301);
    const req = await signedRequest("{}", stale);
    expect(await slackAdapter.verify(req, env(), NOW)).toBe(false);
  });

  it("rejects when the deployment has no signing secret configured", async () => {
    const req = await signedRequest("{}");
    expect(await slackAdapter.verify(req, env({ SLACK_SIGNING_SECRET: undefined }), NOW)).toBe(false);
  });

  it("leaves the body readable afterwards -- router.ts still has to parse it", async () => {
    const req = await signedRequest('{"ok":true}');
    expect(await slackAdapter.verify(req, env(), NOW)).toBe(true);
    expect(await req.json()).toEqual({ ok: true });
  });
});

describe("slack routingKey", () => {
  it("is the team id, for an event_callback", () => {
    const payload = { type: "event_callback", team_id: "T0123", event_id: "Ev1", event: { type: "app_mention" } };
    expect(slackAdapter.routingKey(payload)).toBe("T0123");
  });

  it("is null for url_verification -- it carries no team", () => {
    expect(slackAdapter.routingKey({ type: "url_verification", challenge: "c" })).toBeNull();
  });
});

describe("slack challengeResponse", () => {
  it("extracts the challenge from a url_verification payload", () => {
    expect(slackAdapter.challengeResponse!({ type: "url_verification", challenge: "abc123" })).toBe("abc123");
  });

  it("is null for an ordinary event_callback", () => {
    expect(
      slackAdapter.challengeResponse!({ type: "event_callback", team_id: "T1", event_id: "E1", event: {} }),
    ).toBeNull();
  });

  it("is null for a non-object payload", () => {
    expect(slackAdapter.challengeResponse!(null)).toBeNull();
    expect(slackAdapter.challengeResponse!("a string")).toBeNull();
  });
});

function eventCallback(event: unknown, eventId = "Ev1", teamId = "T0123"): unknown {
  return { type: "event_callback", event_id: eventId, team_id: teamId, event };
}

describe("slack normalize", () => {
  it("normalizes app_mention into one ChannelMessage", async () => {
    const payload = eventCallback({
      type: "app_mention",
      user: "U111",
      text: "<@U0BOT> what's the weather",
      channel: "C222",
      ts: "1700000000.000100",
    });
    const messages = await slackAdapter.normalize(payload, "conn-1", env());
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      v: 1,
      id: "slack:Ev1",
      provider: "slack",
      connection: "conn-1",
      thread: "C222",
      sender: { id: "U111", name: "U111" },
      text: "<@U0BOT> what's the weather",
      raw_kind: "app_mention",
    });
  });

  it("normalizes message.im -- delivered as a plain message event with channel_type im", async () => {
    const payload = eventCallback({
      type: "message",
      channel_type: "im",
      user: "U111",
      text: "hello from a dm",
      channel: "D333",
      ts: "1700000000.000200",
    });
    const messages = await slackAdapter.normalize(payload, "conn-1", env());
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({ thread: "D333", text: "hello from a dm", raw_kind: "message" });
  });

  it("gives a threaded reply the channel:thread_ts thread key", async () => {
    const payload = eventCallback({
      type: "app_mention",
      user: "U111",
      text: "still here?",
      channel: "C222",
      thread_ts: "1699999999.000100",
    });
    const messages = await slackAdapter.normalize(payload, "conn-1", env());
    expect(messages[0].thread).toBe("C222:1699999999.000100");
  });

  it("normalizes assistant_thread_started with an empty text and the thread's own channel/ts", async () => {
    const payload = eventCallback({
      type: "assistant_thread_started",
      assistant_thread: { user_id: "U111", channel_id: "D444", thread_ts: "1700000000.000300" },
    });
    const messages = await slackAdapter.normalize(payload, "conn-1", env());
    expect(messages).toHaveLength(1);
    expect(messages[0]).toMatchObject({
      thread: "D444:1700000000.000300",
      sender: { id: "U111", name: "U111" },
      text: "",
      raw_kind: "assistant_thread_started",
    });
  });

  it("never normalizes this bot's own message -- bot_id would otherwise loop send back into itself", async () => {
    const payload = eventCallback({
      type: "message",
      channel_type: "im",
      bot_id: "B999",
      text: "an echo of our own reply",
      channel: "D333",
    });
    expect(await slackAdapter.normalize(payload, "conn-1", env())).toEqual([]);
  });

  it("never normalizes a subtype event (edit, delete, channel_join, …)", async () => {
    const payload = eventCallback({
      type: "message",
      channel_type: "im",
      subtype: "message_changed",
      channel: "D333",
      user: "U111",
    });
    expect(await slackAdapter.normalize(payload, "conn-1", env())).toEqual([]);
  });

  it("is empty for url_verification -- routingKey already returned null, but normalize is pure", async () => {
    expect(await slackAdapter.normalize({ type: "url_verification", challenge: "c" }, "conn-1", env())).toEqual([]);
  });

  it("carries an image attachment straight through, no round trip needed", async () => {
    const payload = eventCallback({
      type: "app_mention",
      user: "U111",
      text: "look at this",
      channel: "C222",
      files: [{ url_private: "https://files.slack.com/a.png", name: "a.png", mimetype: "image/png" }],
    });
    const messages = await slackAdapter.normalize(payload, "conn-1", env());
    expect(messages[0].attachments).toEqual([
      { kind: "image", url: "https://files.slack.com/a.png", name: "a.png" },
    ]);
  });
});

describe("slack send", () => {
  it("posts to chat.postMessage with the channel split out of the thread key", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true }), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await slackAdapter.send({ routingKey: "T0123", thread: "C222", text: "hi there" }, { botToken: "xoxb-1" });

    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl, init] = fetchMock.mock.calls[0];
    expect(calledUrl).toBe("https://slack.com/api/chat.postMessage");
    expect((init as RequestInit).headers).toMatchObject({ authorization: "Bearer xoxb-1" });
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({ channel: "C222", text: "hi there" });
  });

  it("includes thread_ts once the thread key names a Slack thread", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true }), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await slackAdapter.send(
      { routingKey: "T0123", thread: "C222:1699999999.000100", text: "still working on it" },
      { botToken: "xoxb-1" },
    );

    const [, init] = fetchMock.mock.calls[0];
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      channel: "C222",
      text: "still working on it",
      thread_ts: "1699999999.000100",
    });
  });

  it("throws without a thread -- a team-wide routing key alone names no channel", async () => {
    await expect(
      slackAdapter.send({ routingKey: "T0123", text: "hi" }, { botToken: "xoxb-1" }),
    ).rejects.toThrow(/thread/);
  });

  it("throws when slack rejects the post", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ ok: false, error: "channel_not_found" }), { status: 200 })),
    );
    await expect(
      slackAdapter.send({ routingKey: "T0123", thread: "C222", text: "hi" }, { botToken: "xoxb-1" }),
    ).rejects.toThrow(/channel_not_found/);
  });
});

describe("slack setThinking", () => {
  it("posts to assistant.threads.setStatus with the channel/thread split out", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true }), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    await slackAdapter.setThinking!("C222:1699999999.000100", { botToken: "xoxb-1" }, "thinking…");

    const [calledUrl, init] = fetchMock.mock.calls[0];
    expect(calledUrl).toBe("https://slack.com/api/assistant.threads.setStatus");
    expect(JSON.parse((init as RequestInit).body as string)).toEqual({
      channel_id: "C222",
      thread_ts: "1699999999.000100",
      status: "thinking…",
    });
  });

  it("is a no-op for a bare channel -- there's no thread to set a status on", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    await slackAdapter.setThinking!("C222", { botToken: "xoxb-1" }, "thinking…");
    expect(fetchMock).not.toHaveBeenCalled();
  });
});

describe("slackInstallUrl", () => {
  it("builds the OAuth v2 authorize URL with the link code as state", () => {
    const url = slackInstallUrl("https://router.example/register", env(), "code123");
    expect(url).not.toBeNull();
    const parsed = new URL(url!);
    expect(parsed.origin + parsed.pathname).toBe("https://slack.com/oauth/v2/authorize");
    expect(parsed.searchParams.get("client_id")).toBe("client-id");
    expect(parsed.searchParams.get("state")).toBe("code123");
    expect(parsed.searchParams.get("redirect_uri")).toBe("https://router.example/link/slack");
    expect(parsed.searchParams.get("scope")).toContain("chat:write");
  });

  it("is null without a configured client id", () => {
    expect(slackInstallUrl("https://router.example/register", env({ SLACK_CLIENT_ID: undefined }), "code")).toBeNull();
  });
});

describe("slackRedirectUri", () => {
  it("is /link/slack on the request's own origin", () => {
    expect(slackRedirectUri("https://router.example/register?x=1")).toBe("https://router.example/link/slack");
  });
});

describe("slackExchangeCode", () => {
  it("returns the team id and bot token on a successful exchange", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () =>
        new Response(JSON.stringify({ ok: true, access_token: "xoxb-1", team: { id: "T0123" } }), { status: 200 }),
      ),
    );
    const result = await slackExchangeCode(env(), "oauth-code", "https://router.example/link/slack");
    expect(result).toEqual({ ok: true, teamId: "T0123", botToken: "xoxb-1" });
  });

  it("surfaces slack's own error string on a failed exchange", async () => {
    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ ok: false, error: "invalid_code" }), { status: 200 })),
    );
    const result = await slackExchangeCode(env(), "bad-code", "https://router.example/link/slack");
    expect(result).toEqual({ ok: false, error: "invalid_code" });
  });

  it("fails without client credentials configured, before even calling slack", async () => {
    const fetchMock = vi.fn();
    vi.stubGlobal("fetch", fetchMock);
    const result = await slackExchangeCode(env({ SLACK_CLIENT_SECRET: undefined }), "code", "https://x/link/slack");
    expect(result.ok).toBe(false);
    expect(fetchMock).not.toHaveBeenCalled();
  });
});
