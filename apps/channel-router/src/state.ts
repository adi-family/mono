/**
 * The two signed, expiring tokens this router mints -- the link code and the node token.
 * Neither is backed by a session store; both are self-contained, HMAC-signed payloads in
 * exactly oauth-router's `state.ts` style (sign, compare, check age), over `ROUTER_SECRET`
 * instead of `STATE_SECRET`.
 *
 * - A **link code** (docs/channels.md §1 step 3) rides inside `t.me/<bot>?start=<code>`. It
 *   names the `(node, provider, connection)` a `/start` (or, later, an OAuth callback) is
 *   expected to complete -- short-lived, because it only has to survive one human clicking a
 *   link and sending one message.
 * - A **node token** (§1, §8) is the bearer for `/subscribe` and `/send`, scoped to exactly
 *   `(node id, provider)`. It carries an `epoch`: minting or revoking bumps the epoch the owning
 *   Durable Object has on file, which is what makes a token "signed and expiring" (bad
 *   signature or stale epoch both fail the same way) while still being long-lived and not
 *   rotating on an ordinary reconnect (§2) -- only an explicit mint/revoke moves the epoch.
 */

/** How long a link code stays valid -- one human clicking a link and sending one message. */
export const LINK_CODE_TTL_SECONDS = 600;

/** How long a node token stays valid before it needs re-minting even absent a revoke -- long
 * enough that an always-connected node never notices, short enough that a token nobody ever
 * revoked doesn't outlive every other credential in the system. */
export const NODE_TOKEN_TTL_SECONDS = 400 * 24 * 60 * 60;

export interface LinkCodePayload {
  node: string;
  provider: string;
  connection: string;
  /** Issued-at, epoch seconds. */
  t: number;
}

export interface NodeTokenPayload {
  node: string;
  provider: string;
  epoch: number;
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
  return crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
    "verify",
  ]);
}

/** A URL-safe random id -- used for connection ids. */
export function randomId(): string {
  return bytesToB64url(crypto.getRandomValues(new Uint8Array(16)));
}

async function sign(payload: unknown, secret: string): Promise<string> {
  const body = bytesToB64url(enc.encode(JSON.stringify(payload)));
  const key = await hmacKey(secret);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(body));
  return `${body}.${bytesToB64url(new Uint8Array(sig))}`;
}

/** Verify signature and shape, returning the decoded body or `null` on any failure -- bad
 * base64, bad signature, or a shape check that fails. Doesn't check age; callers that need a
 * TTL check it themselves against the payload's own `t`. */
async function verify(token: string, secret: string): Promise<unknown | null> {
  try {
    const dot = token.indexOf(".");
    if (dot <= 0) return null;
    const body = token.slice(0, dot);
    const sig = b64urlToBytes(token.slice(dot + 1));
    const key = await hmacKey(secret);
    const ok = await crypto.subtle.verify("HMAC", key, sig, enc.encode(body));
    if (!ok) return null;
    return JSON.parse(dec.decode(b64urlToBytes(body)));
  } catch {
    return null;
  }
}

function isFresh(t: unknown, maxAgeSeconds: number, now: number): boolean {
  return typeof t === "number" && now - t <= maxAgeSeconds && t - now <= 60;
}

export async function signLinkCode(payload: LinkCodePayload, secret: string): Promise<string> {
  return sign(payload, secret);
}

export async function verifyLinkCode(
  token: string,
  secret: string,
  now: number,
): Promise<LinkCodePayload | null> {
  const payload = (await verify(token, secret)) as Partial<LinkCodePayload> | null;
  if (!payload) return null;
  if (
    typeof payload.node !== "string" ||
    typeof payload.provider !== "string" ||
    typeof payload.connection !== "string" ||
    !isFresh(payload.t, LINK_CODE_TTL_SECONDS, now)
  ) {
    return null;
  }
  return payload as LinkCodePayload;
}

export async function signNodeToken(payload: NodeTokenPayload, secret: string): Promise<string> {
  return sign(payload, secret);
}

export async function verifyNodeToken(
  token: string,
  secret: string,
  now: number,
): Promise<NodeTokenPayload | null> {
  const payload = (await verify(token, secret)) as Partial<NodeTokenPayload> | null;
  if (!payload) return null;
  if (
    typeof payload.node !== "string" ||
    typeof payload.provider !== "string" ||
    typeof payload.epoch !== "number" ||
    !isFresh(payload.t, NODE_TOKEN_TTL_SECONDS, now)
  ) {
    return null;
  }
  return payload as NodeTokenPayload;
}
