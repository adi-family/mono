/**
 * The OAuth router. One Worker fronts many providers; every flow is two hops through here:
 *
 * 1. `GET /login/<provider>?redirect=<app-url>&scope=<optional>`
 *    → build the provider's authorize URL and 302 the browser to it. What the callback will
 *      need — provider, validated redirect target, CSRF nonce — is packed into a signed
 *      `state` ({@link signState}) and the nonce is also dropped as a cookie.
 *
 * 2. `GET /callback/<provider>?code=&state=`  (the URL registered with the provider)
 *    → verify `state` + the nonce cookie, exchange `code` for a token server-to-server, then
 *      302 the browser back to the app with the token in the URL fragment
 *      ({@link deliveryUrl}). Provider-side errors (`?error=`) are forwarded the same way.
 *
 * The Worker keeps no state between the two hops: trust comes from the signed `state`, not a
 * session store. `GET /` / `GET /health` report which providers are enabled.
 */

import { redirectTo, json, problem } from "./http";
import { enabledProviders, resolveProvider, type ResolvedProvider } from "./providers";
import { allowedRedirect, deliveryUrl } from "./redirect";
import { randomNonce, signState, verifyState } from "./state";
import type { Env } from "./types";

/** How long a signed `state` (and its nonce cookie) stays valid — one login attempt. */
const STATE_TTL_SECONDS = 600;

/** The token endpoint's JSON response — the standard OAuth2 fields, plus OIDC `id_token`. */
interface TokenResponse {
  access_token?: string;
  token_type?: string;
  expires_in?: number;
  refresh_token?: string;
  scope?: string;
  id_token?: string;
  error?: string;
  error_description?: string;
}

/**
 * Route a request. `now` (epoch ms) is injected so tests can pin the clock; production
 * passes the default.
 */
export async function handle(request: Request, env: Env, now: number = Date.now()): Promise<Response> {
  const url = new URL(request.url);
  const segments = url.pathname.split("/").filter(Boolean);

  if (request.method === "GET" && (segments.length === 0 || segments[0] === "health")) {
    return json({ service: "adi-oauth-router", providers: enabledProviders(env) });
  }

  if (request.method === "GET" && segments.length === 2 && segments[0] === "login") {
    return handleLogin(url, segments[1], env, now);
  }

  if (request.method === "GET" && segments.length === 2 && segments[0] === "callback") {
    return handleCallback(url, segments[1], request, env, now);
  }

  if (request.method === "POST" && segments.length === 2 && segments[0] === "refresh") {
    return handleRefresh(segments[1], request, env);
  }

  return problem(404, "not found");
}

/**
 * `POST /refresh/<provider>` with `{ refresh_token }` → mint a fresh access token, server-to-
 * server. This is what lets a stored OAuth secret renew without the user re-authorizing: the
 * app holds the refresh token, the client secret stays here, and only the router talks to the
 * provider. The refresh token is the credential, so no extra auth is imposed. Returns JSON
 * `{ provider, access_token, token_type, expires_in, scope, refresh_token? }` — a provider may
 * rotate the refresh token, so a new one is passed back when present.
 */
async function handleRefresh(providerId: string, request: Request, env: Env): Promise<Response> {
  const provider = resolveProvider(providerId, env);
  if (!provider) return problem(404, `unknown or unconfigured provider: ${providerId}`);

  let body: { refresh_token?: unknown };
  try {
    body = (await request.json()) as { refresh_token?: unknown };
  } catch {
    return problem(400, "expected a JSON body { refresh_token }");
  }
  const refreshToken = typeof body.refresh_token === "string" ? body.refresh_token : "";
  if (!refreshToken) return problem(400, "missing refresh_token");

  const result = await postToken(
    provider,
    new URLSearchParams({
      grant_type: "refresh_token",
      refresh_token: refreshToken,
      client_id: provider.clientId,
      client_secret: provider.clientSecret,
    }),
  );
  if (!result.ok) return problem(502, `token refresh failed: ${result.error}`);

  const token = result.token;
  return json({
    provider: providerId,
    access_token: token.access_token,
    token_type: token.token_type,
    expires_in: token.expires_in,
    scope: token.scope,
    refresh_token: token.refresh_token,
  });
}

/** Build the authorize redirect and set the CSRF nonce cookie. */
async function handleLogin(url: URL, providerId: string, env: Env, now: number): Promise<Response> {
  const provider = resolveProvider(providerId, env);
  if (!provider) return problem(404, `unknown or unconfigured provider: ${providerId}`);

  const redirect = allowedRedirect(url.searchParams.get("redirect"), env);
  if (!redirect) return problem(400, "redirect target is missing or not allow-listed");

  const nonce = randomNonce();
  const state = await signState(
    { p: providerId, r: redirect, n: nonce, t: Math.floor(now / 1000) },
    env.STATE_SECRET,
  );

  const scopeOverride = url.searchParams.get("scope");
  const scopes = scopeOverride ? scopeOverride.split(/[\s,]+/).filter(Boolean) : provider.scopes;

  const authorize = new URL(provider.authUrl);
  const p = authorize.searchParams;
  p.set("client_id", provider.clientId);
  p.set("redirect_uri", callbackUri(url, providerId));
  p.set("response_type", "code");
  p.set("scope", scopes.join(provider.scopeSeparator ?? " "));
  p.set("state", state);
  for (const [k, v] of Object.entries(provider.authParams ?? {})) p.set(k, v);

  return redirectTo(authorize.toString(), nonceCookie(providerId, nonce, isHttps(url)));
}

/** Verify the round-trip, exchange the code, and deliver the token to the app. */
async function handleCallback(
  url: URL,
  providerId: string,
  request: Request,
  env: Env,
  now: number,
): Promise<Response> {
  const provider = resolveProvider(providerId, env);
  if (!provider) return problem(404, `unknown or unconfigured provider: ${providerId}`);

  const clearCookie = clearNonceCookie(providerId, isHttps(url));

  const stateToken = url.searchParams.get("state");
  const state = stateToken
    ? await verifyState(stateToken, env.STATE_SECRET, STATE_TTL_SECONDS, Math.floor(now / 1000))
    : null;
  if (!state || state.p !== providerId) {
    return problem(400, "invalid or expired state", clearCookie);
  }

  // Double-submit CSRF check: the nonce in the signed state must match the cookie we set.
  const cookieNonce = readCookie(request, nonceCookieName(providerId));
  if (!cookieNonce || cookieNonce !== state.n) {
    return problem(400, "state / cookie mismatch", clearCookie);
  }

  // The delivery target is re-validated (env may have changed since login).
  const target = allowedRedirect(state.r, env);
  if (!target) return problem(400, "redirect target is no longer allow-listed", clearCookie);

  // The provider bounced the user back with an error instead of a code — forward it.
  const providerError = url.searchParams.get("error");
  if (providerError) {
    return redirectTo(
      deliveryUrl(target, {
        provider: providerId,
        error: providerError,
        error_description: url.searchParams.get("error_description") ?? undefined,
      }),
      clearCookie,
    );
  }

  const code = url.searchParams.get("code");
  if (!code) return problem(400, "missing authorization code", clearCookie);

  const result = await exchangeCode(provider, code, callbackUri(url, providerId));
  if (!result.ok) {
    return redirectTo(
      deliveryUrl(target, { provider: providerId, error: "token_exchange_failed", error_description: result.error }),
      clearCookie,
    );
  }

  const token = result.token;
  const delivered: Record<string, string | undefined> = {
    provider: providerId,
    access_token: token.access_token,
    token_type: token.token_type,
    expires_in: token.expires_in != null ? String(token.expires_in) : undefined,
    scope: token.scope,
    id_token: token.id_token,
  };
  if (env.INCLUDE_REFRESH_TOKEN === "true" && token.refresh_token) {
    delivered.refresh_token = token.refresh_token;
  }
  return redirectTo(deliveryUrl(target, delivered), clearCookie);
}

/** The result of a token-endpoint POST: the parsed token, or a reason it failed. */
type TokenResult = { ok: true; token: TokenResponse } | { ok: false; error: string };

/** Exchange an authorization code for a token, server-to-server. */
function exchangeCode(
  provider: ResolvedProvider,
  code: string,
  redirectUri: string,
): Promise<TokenResult> {
  return postToken(
    provider,
    new URLSearchParams({
      grant_type: "authorization_code",
      code,
      redirect_uri: redirectUri,
      client_id: provider.clientId,
      client_secret: provider.clientSecret,
    }),
  );
}

/**
 * POST `body` to a provider's token endpoint and parse the JSON token response. Shared by the
 * authorization-code exchange and the refresh-token exchange — both hit the same endpoint with
 * the same content type and the same success/error shape.
 */
async function postToken(provider: ResolvedProvider, body: URLSearchParams): Promise<TokenResult> {
  let res: Response;
  try {
    res = await fetch(provider.tokenUrl, {
      method: "POST",
      headers: {
        "content-type": "application/x-www-form-urlencoded",
        // GitHub returns form-encoded unless asked for JSON; Google always returns JSON.
        accept: "application/json",
        "user-agent": "adi-oauth-router",
      },
      body,
    });
  } catch (e) {
    return { ok: false, error: `network error: ${String(e)}` };
  }

  const text = await res.text();
  let token: TokenResponse;
  try {
    token = JSON.parse(text) as TokenResponse;
  } catch {
    return { ok: false, error: `non-JSON token response (http ${res.status})` };
  }
  if (!res.ok || token.error) {
    return { ok: false, error: token.error_description || token.error || `http ${res.status}` };
  }
  if (!token.access_token) return { ok: false, error: "token response had no access_token" };
  return { ok: true, token };
}

/** This Worker's own callback URL for a provider, derived from the live request origin. */
function callbackUri(url: URL, providerId: string): string {
  return new URL(`/callback/${providerId}`, url.origin).toString();
}

function isHttps(url: URL): boolean {
  return url.protocol === "https:";
}

function nonceCookieName(providerId: string): string {
  return `oauth_nonce_${providerId}`;
}

/** A short-lived, HttpOnly, Lax nonce cookie. `Lax` still rides the top-level callback GET. */
function nonceCookie(providerId: string, nonce: string, secure: boolean): string {
  const attrs = [
    `${nonceCookieName(providerId)}=${nonce}`,
    "HttpOnly",
    "SameSite=Lax",
    "Path=/",
    `Max-Age=${STATE_TTL_SECONDS}`,
  ];
  if (secure) attrs.push("Secure");
  return attrs.join("; ");
}

/** The same cookie, expired — sent on every callback outcome so the nonce never lingers. */
function clearNonceCookie(providerId: string, secure: boolean): string {
  const attrs = [`${nonceCookieName(providerId)}=`, "HttpOnly", "SameSite=Lax", "Path=/", "Max-Age=0"];
  if (secure) attrs.push("Secure");
  return attrs.join("; ");
}

/** Read one cookie value from the request's `Cookie` header. */
function readCookie(request: Request, name: string): string | null {
  const header = request.headers.get("cookie");
  if (!header) return null;
  for (const part of header.split(";")) {
    const eq = part.indexOf("=");
    if (eq < 0) continue;
    if (part.slice(0, eq).trim() === name) return part.slice(eq + 1).trim();
  }
  return null;
}
