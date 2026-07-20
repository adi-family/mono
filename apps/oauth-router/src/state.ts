/**
 * The OAuth `state` parameter, made trustworthy without server-side storage.
 *
 * A Worker has no session by default, yet the callback must know three things the login
 * request chose: which provider this is, where to deliver the token, and a nonce to bind
 * against a cookie. We carry those across the round-trip *through the user's browser* — so
 * they must be unforgeable. Each field lives in a compact payload that is HMAC-signed with
 * {@link Env.STATE_SECRET} and stamped with an issue time; {@link verifyState} rejects any
 * token whose signature doesn't match or whose age exceeds the TTL. The value is opaque and
 * integrity-protected, not encrypted — it holds no secret, only a signed redirect target.
 */

/** The claims a signed `state` carries from `/login` to `/callback`. Field names are kept
 * short only to keep the encoded token small. */
export interface StatePayload {
  /** Provider id the flow was started for; must match the `/callback/<provider>` path. */
  p: string;
  /** Validated, absolute token-delivery target (already checked against the allow-list). */
  r: string;
  /** Random nonce, also set as a cookie for a double-submit CSRF check. */
  n: string;
  /** Issued-at, epoch seconds. */
  t: number;
}

const enc = new TextEncoder();
const dec = new TextDecoder();

function bytesToB64url(bytes: Uint8Array): string {
  let bin = "";
  for (const b of bytes) bin += String.fromCharCode(b);
  return btoa(bin).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function b64urlToBytes(s: string): Uint8Array<ArrayBuffer> {
  const pad = s.length % 4 === 0 ? "" : "=".repeat(4 - (s.length % 4));
  const bin = atob(s.replace(/-/g, "+").replace(/_/g, "/") + pad);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

async function hmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey(
    "raw",
    enc.encode(secret),
    { name: "HMAC", hash: "SHA-256" },
    false,
    ["sign", "verify"],
  );
}

/** A URL-safe random nonce (128 bits), used for the double-submit CSRF cookie. */
export function randomNonce(): string {
  return bytesToB64url(crypto.getRandomValues(new Uint8Array(16)));
}

/** Sign a payload into `<body>.<sig>`, both base64url. */
export async function signState(payload: StatePayload, secret: string): Promise<string> {
  const body = bytesToB64url(enc.encode(JSON.stringify(payload)));
  const key = await hmacKey(secret);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(body));
  return `${body}.${bytesToB64url(new Uint8Array(sig))}`;
}

/**
 * Verify signature and freshness, returning the payload or `null`. `null` covers every
 * failure — malformed token, bad signature, expired, or a clock far in the future — so a
 * caller only has to check for `null` to reject a callback. `now` is epoch seconds, taken
 * from the caller so it can be pinned in tests.
 */
export async function verifyState(
  token: string,
  secret: string,
  maxAgeSeconds: number,
  now: number,
): Promise<StatePayload | null> {
  try {
    const dot = token.indexOf(".");
    if (dot <= 0) return null;
    const body = token.slice(0, dot);
    const sig = b64urlToBytes(token.slice(dot + 1));
    const key = await hmacKey(secret);
    const ok = await crypto.subtle.verify("HMAC", key, sig, enc.encode(body));
    if (!ok) return null;

    const payload = JSON.parse(dec.decode(b64urlToBytes(body))) as StatePayload;
    if (
      typeof payload.p !== "string" ||
      typeof payload.r !== "string" ||
      typeof payload.n !== "string" ||
      typeof payload.t !== "number"
    ) {
      return null;
    }
    // Reject stale tokens, and tokens stamped in the future beyond small clock skew.
    if (now - payload.t > maxAgeSeconds || payload.t - now > 60) return null;
    return payload;
  } catch {
    return null;
  }
}
