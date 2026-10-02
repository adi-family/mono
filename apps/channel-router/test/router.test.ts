import { describe, it, expect, vi, afterEach, beforeAll } from "vitest";
import { env } from "cloudflare:test";

import { handle } from "../src/router";
import { migrate } from "./migrate";

beforeAll(migrate);

const NOW = 1_700_000_000_000;

function registerRequest(body: Record<string, unknown>): Request {
  return new Request("https://router.example/register", {
    method: "POST",
    headers: { authorization: `Bearer ${env.ROUTER_ADMIN_SECRET}` },
    body: JSON.stringify(body),
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("health", () => {
  it("reports both telegram and slack enabled once both are configured", async () => {
    const res = await handle(new Request("https://router.example/health"), env, NOW);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ service: "adi-channel-router", providers: ["telegram", "slack"] });
  });

  it("GET / answers the same as /health", async () => {
    const res = await handle(new Request("https://router.example/"), env, NOW);
    expect(await res.json()).toMatchObject({ service: "adi-channel-router" });
  });
});

describe("POST /register", () => {
  it("401s without a bearer", async () => {
    const res = await handle(
      new Request("https://router.example/register", { method: "POST", body: "{}" }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });

  it("401s the wrong admin secret", async () => {
    const res = await handle(
      new Request("https://router.example/register", {
        method: "POST",
        headers: { authorization: "Bearer wrong" },
        body: "{}",
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });

  it("400s a missing target", async () => {
    const res = await handle(
      registerRequest({ node: "node-1", provider: "telegram" }),
      env,
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("404s an unknown or unsupported provider", async () => {
    const res = await handle(
      registerRequest({ node: "node-1", provider: "discord", target: { kind: "agent", agent: "a" } }),
      env,
      NOW,
    );
    expect(res.status).toBe(404);
  });

  it("mints a token, a connection, and a link code for telegram", async () => {
    const res = await handle(
      registerRequest({
        node: `node-register-${Math.random()}`,
        provider: "telegram",
        target: { kind: "agent", agent: "my-agent" },
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    const body = (await res.json()) as { token: string; connection: string; link_code: string; install_url: string };
    expect(body.token.length).toBeGreaterThan(0);
    expect(body.connection.length).toBeGreaterThan(0);
    // Telegram's own limit on a deep link's `start` parameter; anything else is silently dropped
    // and the link never completes (ADI-MONO-124 found this against the real bot).
    expect(body.link_code).toMatch(/^[A-Za-z0-9_-]{1,64}$/);
    expect(body.install_url).toBe(`https://t.me/TestAdiBot?start=${body.link_code}`);
  });

  it("reuses the same node token across two connections, but mints a fresh connection/code each time", async () => {
    const node = `node-reuse-${Math.random()}`;
    const first = (await (
      await handle(registerRequest({ node, provider: "telegram", target: { kind: "agent", agent: "a" } }), env, NOW)
    ).json()) as { token: string; connection: string; link_code: string };
    const second = (await (
      await handle(registerRequest({ node, provider: "telegram", target: { kind: "agent", agent: "b" } }), env, NOW)
    ).json()) as { token: string; connection: string; link_code: string };

    expect(second.token).toBe(first.token);
    expect(second.connection).not.toBe(first.connection);
    expect(second.link_code).not.toBe(first.link_code);
  });
});

describe("GET /link/<provider>", () => {
  it("404s telegram -- it links through its webhook, not a redirect", async () => {
    const res = await handle(new Request("https://router.example/link/telegram?code=x"), env, NOW);
    expect(res.status).toBe(404);
  });

  it("404s an unknown provider", async () => {
    const res = await handle(new Request("https://router.example/link/discord?code=x"), env, NOW);
    expect(res.status).toBe(404);
  });
});

describe("POST /webhook/<provider> -- signature check", () => {
  it("400s a missing secret token header", async () => {
    const res = await handle(
      new Request("https://router.example/webhook/telegram", {
        method: "POST",
        body: JSON.stringify({ update_id: 1, message: { message_id: 1, chat: { id: 1 }, text: "hi" } }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("400s a wrong secret token header", async () => {
    const res = await handle(
      new Request("https://router.example/webhook/telegram", {
        method: "POST",
        headers: { "x-telegram-bot-api-secret-token": "nope" },
        body: JSON.stringify({ update_id: 1, message: { message_id: 1, chat: { id: 1 }, text: "hi" } }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("404s an unknown provider before even checking a signature", async () => {
    const res = await handle(
      new Request("https://router.example/webhook/discord", { method: "POST", body: "{}" }),
      env,
      NOW,
    );
    expect(res.status).toBe(404);
  });

  it("200s (and does nothing) a correctly-signed webhook for a chat nobody has linked", async () => {
    const res = await handle(
      new Request("https://router.example/webhook/telegram", {
        method: "POST",
        headers: { "x-telegram-bot-api-secret-token": env.TELEGRAM_SECRET_TOKEN! },
        body: JSON.stringify({
          update_id: 1,
          message: { message_id: 1, chat: { id: 999_999 }, text: "hello, nobody linked me" },
        }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
  });
});

describe("POST /subscribe -- auth", () => {
  it("401s a missing token", async () => {
    const res = await handle(new Request("https://router.example/subscribe"), env, NOW);
    expect(res.status).toBe(401);
  });

  it("401s an invalid token", async () => {
    const res = await handle(
      new Request("https://router.example/subscribe", { headers: { authorization: "Bearer garbage" } }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });
});

describe("POST /send -- auth", () => {
  it("401s a missing token", async () => {
    const res = await handle(
      new Request("https://router.example/send", { method: "POST", body: "{}" }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });

  it("401s an invalid token", async () => {
    const res = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: "Bearer garbage" },
        body: "{}",
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });

  it("400s a well-authenticated request missing { connection, text }", async () => {
    const node = `node-send-shape-${Math.random()}`;
    const { token } = (await (
      await handle(registerRequest({ node, provider: "telegram", target: { kind: "agent", agent: "a" } }), env, NOW)
    ).json()) as { token: string };

    const res = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ text: "hi" }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("404s a valid token against a connection that was never linked", async () => {
    const node = `node-send-unlinked-${Math.random()}`;
    const { token, connection } = (await (
      await handle(registerRequest({ node, provider: "telegram", target: { kind: "agent", agent: "a" } }), env, NOW)
    ).json()) as { token: string; connection: string };

    const res = await handle(
      new Request("https://router.example/send", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ connection, text: "hi" }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(404);
  });
});

async function signedSlackRequest(bodyObj: unknown, timestamp = String(NOW / 1000)): Promise<Request> {
  const body = JSON.stringify(bodyObj);
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

describe("Slack install + OAuth link", () => {
  it("register's install_url is the OAuth v2 authorize URL, state carrying the link code", async () => {
    const node = `node-slack-install-${Math.random()}`;
    const res = await handle(
      registerRequest({ node, provider: "slack", target: { kind: "agent", agent: "a" } }),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    const body = (await res.json()) as { link_code: string; install_url: string };
    const url = new URL(body.install_url);
    expect(url.origin + url.pathname).toBe("https://slack.com/oauth/v2/authorize");
    expect(url.searchParams.get("state")).toBe(body.link_code);
    expect(url.searchParams.get("redirect_uri")).toBe("https://router.example/link/slack");
  });

  it("completes the link, stores the routing key, and the token exchange happened", async () => {
    const node = `node-slack-link-${Math.random()}`;
    const { link_code } = (await (
      await handle(
        registerRequest({ node, provider: "slack", target: { kind: "agent", agent: "a" } }),
        env,
        NOW,
      )
    ).json()) as { link_code: string };

    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      new Response(JSON.stringify({ ok: true, access_token: "xoxb-1", team: { id: "T0123" } }), { status: 200 }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const res = await handle(
      new Request(`https://router.example/link/slack?code=oauth-code&state=${encodeURIComponent(link_code)}`),
      env,
      NOW,
    );
    expect(res.status).toBe(200);
    expect(await res.text()).toContain("Linked");
    expect(fetchMock).toHaveBeenCalledWith(
      "https://slack.com/api/oauth.v2.access",
      expect.anything(),
    );

    const row = await env.ROUTING_KEYS.prepare("SELECT node_id FROM routing_keys WHERE provider = ? AND routing_key = ?")
      .bind("slack", "T0123")
      .first<{ node_id: string }>();
    expect(row?.node_id).toBe(node);
  });

  it("400s an invalid or missing state", async () => {
    const res = await handle(
      new Request("https://router.example/link/slack?code=oauth-code&state=forged"),
      env,
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("502s when slack's token exchange fails", async () => {
    const node = `node-slack-exchange-fail-${Math.random()}`;
    const { link_code } = (await (
      await handle(
        registerRequest({ node, provider: "slack", target: { kind: "agent", agent: "a" } }),
        env,
        NOW,
      )
    ).json()) as { link_code: string };

    vi.stubGlobal(
      "fetch",
      vi.fn(async () => new Response(JSON.stringify({ ok: false, error: "invalid_code" }), { status: 200 })),
    );
    const res = await handle(
      new Request(`https://router.example/link/slack?code=bad&state=${encodeURIComponent(link_code)}`),
      env,
      NOW,
    );
    expect(res.status).toBe(502);
  });
});

describe("POST /webhook/slack", () => {
  it("echoes the url_verification challenge verbatim", async () => {
    const req = await signedSlackRequest({ type: "url_verification", challenge: "abc123" });
    const res = await handle(req, env, NOW);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ challenge: "abc123" });
  });

  it("400s a wrong signature", async () => {
    const req = await signedSlackRequest({ type: "url_verification", challenge: "abc123" }, "1");
    const res = await handle(req, env, NOW);
    expect(res.status).toBe(400);
  });

  it("200s (and does nothing) a correctly-signed event for a workspace nobody has linked", async () => {
    const req = await signedSlackRequest({
      type: "event_callback",
      event_id: "Ev1",
      team_id: "T_UNLINKED",
      event: { type: "app_mention", user: "U1", text: "hi", channel: "C1" },
    });
    const res = await handle(req, env, NOW);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ ok: true });
  });
});

describe("POST /disconnect -- auth", () => {
  it("401s a missing token", async () => {
    const res = await handle(
      new Request("https://router.example/disconnect", { method: "POST", body: "{}" }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });

  it("401s an invalid token", async () => {
    const res = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: "Bearer garbage" },
        body: "{}",
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(401);
  });

  it("400s a well-authenticated request missing { connection }", async () => {
    const node = `node-disconnect-shape-${Math.random()}`;
    const { token } = (await (
      await handle(registerRequest({ node, provider: "telegram", target: { kind: "agent", agent: "a" } }), env, NOW)
    ).json()) as { token: string };

    const res = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: "{}",
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("404s a valid token against an unknown connection", async () => {
    const node = `node-disconnect-unknown-${Math.random()}`;
    const { token } = (await (
      await handle(registerRequest({ node, provider: "telegram", target: { kind: "agent", agent: "a" } }), env, NOW)
    ).json()) as { token: string };

    const res = await handle(
      new Request("https://router.example/disconnect", {
        method: "POST",
        headers: { authorization: `Bearer ${token}` },
        body: JSON.stringify({ connection: "no-such-connection" }),
      }),
      env,
      NOW,
    );
    expect(res.status).toBe(404);
  });
});
