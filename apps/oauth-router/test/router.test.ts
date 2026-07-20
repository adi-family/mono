import { describe, it, expect, vi, afterEach } from "vitest";

import { handle } from "../src/router";
import type { Env } from "../src/types";

const NOW = 1_700_000_000_000; // fixed epoch ms so signed state is deterministic

function env(overrides: Partial<Env> = {}): Env {
  return {
    STATE_SECRET: "test-secret",
    APP_URL: "https://app.example",
    GOOGLE_CLIENT_ID: "gid",
    GOOGLE_CLIENT_SECRET: "gsecret",
    ...overrides,
  } as Env;
}

/** Run a login and pull out the signed state and the nonce cookie it set. */
async function login(e: Env, path = "https://router.example/login/google"): Promise<{
  location: URL;
  state: string;
  nonce: string;
}> {
  const res = await handle(new Request(path), e, NOW);
  expect(res.status).toBe(302);
  const location = new URL(res.headers.get("location")!);
  const state = location.searchParams.get("state")!;
  const setCookie = res.headers.get("set-cookie")!;
  const nonce = /oauth_nonce_google=([^;]+)/.exec(setCookie)![1];
  return { location, state, nonce };
}

function tokenResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("health", () => {
  it("reports the enabled providers", async () => {
    const res = await handle(new Request("https://router.example/health"), env(), NOW);
    expect(res.status).toBe(200);
    expect(await res.json()).toEqual({ service: "adi-oauth-router", providers: ["google"] });
  });
});

describe("login", () => {
  it("redirects to Google's authorize endpoint with the expected params", async () => {
    const { location, state } = await login(env());
    expect(location.origin + location.pathname).toBe("https://accounts.google.com/o/oauth2/v2/auth");
    expect(location.searchParams.get("client_id")).toBe("gid");
    expect(location.searchParams.get("redirect_uri")).toBe("https://router.example/callback/google");
    expect(location.searchParams.get("response_type")).toBe("code");
    expect(location.searchParams.get("scope")).toBe("openid email profile");
    expect(location.searchParams.get("access_type")).toBe("offline");
    expect(state.length).toBeGreaterThan(0);
  });

  it("404s an unconfigured provider", async () => {
    const res = await handle(new Request("https://router.example/login/github"), env(), NOW);
    expect(res.status).toBe(404);
  });

  it("400s a redirect target that is not allow-listed", async () => {
    const res = await handle(
      new Request("https://router.example/login/google?redirect=https://evil.example/"),
      env(),
      NOW,
    );
    expect(res.status).toBe(400);
  });
});

describe("callback", () => {
  it("exchanges the code and redirects the token back to the app", async () => {
    const e = env();
    const { state, nonce } = await login(e);

    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      tokenResponse({ access_token: "AT", token_type: "Bearer", expires_in: 3600, scope: "openid email" }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const res = await handle(
      new Request(`https://router.example/callback/google?code=CODE&state=${encodeURIComponent(state)}`, {
        headers: { cookie: `oauth_nonce_google=${nonce}` },
      }),
      e,
      NOW + 5_000,
    );

    expect(res.status).toBe(302);
    const dest = new URL(res.headers.get("location")!);
    expect(dest.origin + dest.pathname).toBe("https://app.example/");
    const frag = new URLSearchParams(dest.hash.slice(1));
    expect(frag.get("provider")).toBe("google");
    expect(frag.get("access_token")).toBe("AT");
    expect(frag.get("token_type")).toBe("Bearer");
    expect(frag.get("expires_in")).toBe("3600");

    // The exchange hit Google's token endpoint with the code.
    expect(fetchMock).toHaveBeenCalledOnce();
    const [calledUrl] = fetchMock.mock.calls[0];
    expect(calledUrl).toBe("https://oauth2.googleapis.com/token");
  });

  it("keeps the refresh_token out of the fragment unless opted in", async () => {
    const e = env();
    const { state, nonce } = await login(e);
    vi.stubGlobal("fetch", vi.fn(async () => tokenResponse({ access_token: "AT", refresh_token: "RT" })));

    const res = await handle(
      new Request(`https://router.example/callback/google?code=CODE&state=${encodeURIComponent(state)}`, {
        headers: { cookie: `oauth_nonce_google=${nonce}` },
      }),
      e,
      NOW + 5_000,
    );
    const frag = new URLSearchParams(new URL(res.headers.get("location")!).hash.slice(1));
    expect(frag.has("refresh_token")).toBe(false);
  });

  it("forwards the refresh_token when INCLUDE_REFRESH_TOKEN=true", async () => {
    const e = env({ INCLUDE_REFRESH_TOKEN: "true" });
    const { state, nonce } = await login(e);
    vi.stubGlobal("fetch", vi.fn(async () => tokenResponse({ access_token: "AT", refresh_token: "RT" })));

    const res = await handle(
      new Request(`https://router.example/callback/google?code=CODE&state=${encodeURIComponent(state)}`, {
        headers: { cookie: `oauth_nonce_google=${nonce}` },
      }),
      e,
      NOW + 5_000,
    );
    const frag = new URLSearchParams(new URL(res.headers.get("location")!).hash.slice(1));
    expect(frag.get("refresh_token")).toBe("RT");
  });

  it("rejects a callback whose state is forged", async () => {
    const res = await handle(
      new Request("https://router.example/callback/google?code=CODE&state=forged", {
        headers: { cookie: "oauth_nonce_google=whatever" },
      }),
      env(),
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("rejects a callback whose nonce cookie is missing", async () => {
    const e = env();
    const { state } = await login(e);
    const res = await handle(
      new Request(`https://router.example/callback/google?code=CODE&state=${encodeURIComponent(state)}`),
      e,
      NOW + 5_000,
    );
    expect(res.status).toBe(400);
  });

  it("forwards a provider-side error to the app as a fragment", async () => {
    const e = env();
    const { state, nonce } = await login(e);
    const res = await handle(
      new Request(
        `https://router.example/callback/google?error=access_denied&state=${encodeURIComponent(state)}`,
        { headers: { cookie: `oauth_nonce_google=${nonce}` } },
      ),
      e,
      NOW + 5_000,
    );
    expect(res.status).toBe(302);
    const frag = new URLSearchParams(new URL(res.headers.get("location")!).hash.slice(1));
    expect(frag.get("error")).toBe("access_denied");
    expect(frag.get("provider")).toBe("google");
  });

  it("redirects with an error when the token exchange fails", async () => {
    const e = env();
    const { state, nonce } = await login(e);
    vi.stubGlobal("fetch", vi.fn(async () => tokenResponse({ error: "invalid_grant" }, 400)));

    const res = await handle(
      new Request(`https://router.example/callback/google?code=CODE&state=${encodeURIComponent(state)}`, {
        headers: { cookie: `oauth_nonce_google=${nonce}` },
      }),
      e,
      NOW + 5_000,
    );
    expect(res.status).toBe(302);
    const frag = new URLSearchParams(new URL(res.headers.get("location")!).hash.slice(1));
    expect(frag.get("error")).toBe("token_exchange_failed");
  });
});

describe("refresh", () => {
  it("exchanges a refresh token for a fresh access token", async () => {
    const fetchMock = vi.fn(async (_url: string, _init?: RequestInit) =>
      tokenResponse({ access_token: "AT2", token_type: "Bearer", expires_in: 3600, refresh_token: "RT2" }),
    );
    vi.stubGlobal("fetch", fetchMock);

    const res = await handle(
      new Request("https://router.example/refresh/google", {
        method: "POST",
        body: JSON.stringify({ refresh_token: "RT1" }),
      }),
      env(),
      NOW,
    );
    expect(res.status).toBe(200);
    expect(await res.json()).toMatchObject({
      provider: "google",
      access_token: "AT2",
      expires_in: 3600,
      refresh_token: "RT2",
    });

    // It hit the token endpoint with grant_type=refresh_token + the refresh token.
    expect(fetchMock).toHaveBeenCalledOnce();
    const body = fetchMock.mock.calls[0][1]!.body as URLSearchParams;
    expect(body.get("grant_type")).toBe("refresh_token");
    expect(body.get("refresh_token")).toBe("RT1");
  });

  it("400s a missing refresh_token", async () => {
    const res = await handle(
      new Request("https://router.example/refresh/google", { method: "POST", body: "{}" }),
      env(),
      NOW,
    );
    expect(res.status).toBe(400);
  });

  it("404s an unconfigured provider", async () => {
    const res = await handle(
      new Request("https://router.example/refresh/github", {
        method: "POST",
        body: JSON.stringify({ refresh_token: "x" }),
      }),
      env(),
      NOW,
    );
    expect(res.status).toBe(404);
  });

  it("502s when the provider rejects the refresh", async () => {
    vi.stubGlobal("fetch", vi.fn(async () => tokenResponse({ error: "invalid_grant" }, 400)));
    const res = await handle(
      new Request("https://router.example/refresh/google", {
        method: "POST",
        body: JSON.stringify({ refresh_token: "stale" }),
      }),
      env(),
      NOW,
    );
    expect(res.status).toBe(502);
  });
});
