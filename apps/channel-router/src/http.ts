/**
 * Tiny response constructors, same convention as oauth-router's `http.ts`: everything this
 * router returns is uncacheable, so `Cache-Control: no-store` is baked in here rather than
 * remembered at each call site.
 */

const NO_STORE = "no-store";

/** A JSON body with the given status. */
export function json(data: unknown, status = 200): Response {
  return new Response(JSON.stringify(data), {
    status,
    headers: { "content-type": "application/json; charset=utf-8", "cache-control": NO_STORE },
  });
}

/** A plain-text error. */
export function problem(status: number, message: string): Response {
  return new Response(`${message}\n`, {
    status,
    headers: { "content-type": "text/plain; charset=utf-8", "cache-control": NO_STORE },
  });
}

/** A small HTML page -- the browser-facing end of a redirect-based install flow (Slack's OAuth
 * callback, §4), which answers the human's tab directly rather than a service's webhook retry
 * logic the way every other route here does. */
export function html(body: string, status = 200): Response {
  return new Response(body, {
    status,
    headers: { "content-type": "text/html; charset=utf-8", "cache-control": NO_STORE },
  });
}

/** Constant-time string compare -- used for the admin bearer and Telegram's secret token, so
 * neither is checked with a comparison whose timing leaks how much of it matched. */
export function timingSafeEqual(a: string, b: string): boolean {
  const enc = new TextEncoder();
  const ab = enc.encode(a);
  const bb = enc.encode(b);
  // A length mismatch is itself safe to short-circuit on: both strings are of a fixed, known
  // size in practice (a secret, a bot's configured token), so leaking "wrong length" leaks
  // nothing an attacker didn't already know how to probe for some other way.
  if (ab.length !== bb.length) return false;
  let diff = 0;
  for (let i = 0; i < ab.length; i++) diff |= ab[i] ^ bb[i];
  return diff === 0;
}

/** Read a bearer token from `Authorization: Bearer <token>`, or `null`. */
export function bearerToken(request: Request): string | null {
  const header = request.headers.get("authorization");
  if (!header) return null;
  const match = /^Bearer (.+)$/.exec(header);
  return match ? match[1] : null;
}
