/**
 * Where a token is allowed to go, and how it gets there.
 *
 * "Route the token to the user" means: after the code is exchanged, redirect the browser
 * back to the app with the token attached. Two safety rules live here:
 *
 * 1. The redirect target's **origin must be allow-listed** ({@link allowedRedirect}) — an
 *    open redirect here would let anyone turn this Worker into a token-stealing endpoint.
 * 2. The token is delivered in the URL **fragment** ({@link deliveryUrl}), never the query
 *    — fragments aren't sent to servers, don't land in access logs, and don't leak via
 *    `Referer`. The app reads it from `location.hash`.
 *
 * The transport-security guard requires `https:` for public origins, but the app that
 * consumes the token here is the **local** ADI app served over plain http on the `.adi`
 * split-DNS zone (e.g. `http://app.adi/...`). So http is permitted for the local `.adi`
 * zone and loopback ({@link allowsInsecure}); those hosts are only reachable on the trusted
 * local network. The origin allow-list still gates them — `.adi` alone isn't a free pass.
 */

import type { Env } from "./types";

/**
 * Whether a plain-http redirect target is acceptable for this hostname: the local ADI
 * `.adi` split-DNS zone (e.g. `app.adi`) and loopback. Everything else must be https.
 */
function allowsInsecure(hostname: string): boolean {
  return (
    hostname === "localhost" ||
    hostname === "127.0.0.1" ||
    hostname === "::1" ||
    hostname === "adi" ||
    hostname.endsWith(".adi")
  );
}

/** The set of origins a redirect may target: always `APP_URL`, plus `ALLOWED_REDIRECT_ORIGINS`. */
function allowedOrigins(env: Env): Set<string> {
  const origins = new Set<string>();
  if (env.APP_URL) {
    try {
      origins.add(new URL(env.APP_URL).origin);
    } catch {
      // A malformed APP_URL just contributes no origin; callers still fail closed.
    }
  }
  for (const raw of (env.ALLOWED_REDIRECT_ORIGINS ?? "").split(",")) {
    const t = raw.trim();
    if (!t) continue;
    try {
      origins.add(new URL(t).origin);
    } catch {
      // Ignore an unparsable entry rather than failing the whole deployment.
    }
  }
  return origins;
}

/**
 * Validate a caller-supplied redirect target. Returns the absolute URL to redirect to, or
 * `null` if it isn't allowed. A missing `candidate` falls back to `APP_URL`. A candidate is
 * accepted only if it parses, is https (or http on the local `.adi` zone / loopback — see
 * {@link allowsInsecure}), and its origin is on the allow-list. Any path under an allowed
 * origin is fine — the gate is the origin, so `http://app.adi/anything/here` is accepted
 * once `http://app.adi` is allow-listed.
 */
export function allowedRedirect(candidate: string | null | undefined, env: Env): string | null {
  if (!candidate) return env.APP_URL || null;

  let u: URL;
  try {
    u = new URL(candidate);
  } catch {
    return null;
  }

  if (u.protocol !== "https:" && !(u.protocol === "http:" && allowsInsecure(u.hostname))) {
    return null;
  }

  return allowedOrigins(env).has(u.origin) ? u.toString() : null;
}

/**
 * Build the final redirect URL, attaching `params` to the fragment. Undefined/empty values
 * are dropped, and any pre-existing fragment on `target` is replaced.
 */
export function deliveryUrl(target: string, params: Record<string, string | undefined>): string {
  const u = new URL(target);
  const frag = new URLSearchParams();
  for (const [k, v] of Object.entries(params)) {
    if (v != null && v !== "") frag.set(k, v);
  }
  u.hash = frag.toString();
  return u.toString();
}
