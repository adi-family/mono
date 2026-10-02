/**
 * The Slack adapter -- docs/channels.md §4, "Slack, worked through it", plus the
 * `slack-agentexchange-listing-2026` research report's §3 (the Agents & AI Apps surface's exact
 * scopes/events) and §4 (zero-copy, no Socket Mode in the public Marketplace).
 *
 * - `verify`: HMAC-SHA256 `v0:<timestamp>:<raw body>` against `SLACK_SIGNING_SECRET`, timestamp
 *   rejected past five minutes old -- Slack's own documented scheme.
 * - `routingKey`: `team_id` -- a **workspace**, not a channel (§4; §5's thread map narrows it
 *   back down per-channel).
 * - `challengeResponse`: the `url_verification` handshake Slack sends once, at Request-URL
 *   setup time, which has to be echoed back verbatim rather than routed anywhere.
 * - `normalize`: `assistant_thread_started`, `message.im` (delivered as a plain `message` event
 *   with `channel_type: "im"` -- Slack's event *subscription* names are finer-grained than its
 *   event *payload* types), and `app_mention`, each into zero-or-one `ChannelMessage`. `id` is
 *   Slack's own `event_id` verbatim -- the dedup key its own retry policy (up to 3 redeliveries
 *   of the same `event_id`) makes load-bearing, not hypothetical.
 * - `send`: `chat.postMessage` with the workspace's own bot token (§4 -- one app, many
 *   workspaces, each handing back its own token at OAuth time, unlike Telegram's one shared
 *   secret). `setThinking`: `assistant.threads.setStatus`, the Agents & AI Apps "thinking…"
 *   indicator.
 * - `resolveSenderName`: `users.info`, closing the gap ADI-MONO-123 flagged and left --
 *   `normalize` has no credential to reach, so `router.ts` calls this afterwards, once it has
 *   fetched the connection and resolved its bot token anyway. Cached in-memory, keyed by the bot
 *   token and sender id together (so two workspaces never collide on the same short Slack user
 *   id), for `SENDER_NAME_CACHE_TTL_MS`.
 * - install/link: real OAuth (`GET /link/slack` *is* used here, unlike Telegram) -- see
 *   `slackInstallUrl`/`slackExchangeCode`, called from `router.ts` rather than through the
 *   `ChannelAdapter` interface, same precedent `telegramInstallUrl` already set.
 */
import type { ChannelAdapter, ChannelMessage, Env, OutboundReply } from "../types";
import { timingSafeEqual } from "../http";

export interface SlackCredential {
  botToken: string;
}

/** The bot scopes this app asks for at install time -- see `slack-manifest.yaml`'s
 * `oauth_config.scopes.bot`, which must list exactly these. `app_mentions:read` for the
 * `app_mention` event, `assistant:write` for the Agents & AI Apps surface (`assistant_thread_*`
 * events, `assistant.threads.setStatus`), `chat:write` to reply, `im:history` for `message.im`,
 * `users:read` for `resolveSenderName`'s `users.info` call. */
const SLACK_BOT_SCOPES = ["app_mentions:read", "assistant:write", "chat:write", "im:history", "users:read"];

/** How far a webhook's `X-Slack-Request-Timestamp` may drift from this router's own clock before
 * `verify` refuses it -- Slack's own documented replay window. */
const SIGNATURE_MAX_AGE_SECONDS = 300;

/** How long a resolved display name is trusted before `resolveSenderName` asks `users.info`
 * again -- long enough that a chatty thread doesn't re-resolve on every message, short enough
 * that a renamed account catches up the same day. */
const SENDER_NAME_CACHE_TTL_MS = 10 * 60 * 1000;

/** In-memory only -- a cold start just means the next message re-resolves (§8: zero-copy, this
 * holds nothing a redeploy or eviction couldn't lose for free). Keyed by `<bot token>:<user id>`
 * rather than the user id alone: two different workspaces each have their own bot token, so this
 * can never hand one workspace's cached name back for another's (Enterprise Grid shared-channel)
 * same-looking id. */
const senderNameCache = new Map<string, { name: string; expiresAt: number }>();

interface SlackEventBase {
  type: string;
  user?: string;
  text?: string;
  ts?: string;
  channel?: string;
  channel_type?: string;
  thread_ts?: string;
  /** Present on a message this *bot* sent -- including its own replies echoing back through
   * `message.im`. Never normalized: without this check, `send`'s own `chat.postMessage` would
   * re-trigger the webhook it was answering, forever. */
  bot_id?: string;
  /** Present on anything that isn't a plain user message (`bot_message`, `message_changed`,
   * `channel_join`, …) -- skipped the same way a `bot_id` message is, for the same reason. */
  subtype?: string;
  files?: Array<{ url_private?: string; name?: string; mimetype?: string }>;
}

interface SlackAssistantThreadEvent {
  type: "assistant_thread_started";
  assistant_thread: {
    user_id: string;
    channel_id: string;
    thread_ts: string;
  };
}

type SlackEvent = SlackEventBase | SlackAssistantThreadEvent;

interface SlackEventCallback {
  type: "event_callback";
  event_id: string;
  team_id: string;
  event: SlackEvent;
}

interface SlackUrlVerification {
  type: "url_verification";
  challenge: string;
}

type SlackEnvelope = SlackEventCallback | SlackUrlVerification | { type: string; team_id?: string };

function asEventCallback(payload: unknown): SlackEventCallback | null {
  const envelope = payload as SlackEnvelope;
  return envelope?.type === "event_callback" ? (envelope as SlackEventCallback) : null;
}

/** The provider thread key (§5): the channel alone, or `channel:thread_ts` once a reply has
 * opened a Slack thread -- `send`'s own parsing of `OutboundReply.thread` is the exact inverse. */
function threadKey(channel: string, threadTs?: string): string {
  return threadTs ? `${channel}:${threadTs}` : channel;
}

/** Whether `event` is something worth forwarding -- not this bot's own echo, and not a subtype
 * (edit, delete, channel-join, …) with no human text to react to. */
function isOrdinaryUserEvent(event: SlackEventBase): boolean {
  return !event.bot_id && !event.subtype;
}

async function hmacSha256Hex(secret: string, message: string): Promise<string> {
  const enc = new TextEncoder();
  const key = await crypto.subtle.importKey("raw", enc.encode(secret), { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  const sig = await crypto.subtle.sign("HMAC", key, enc.encode(message));
  return Array.from(new Uint8Array(sig))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
}

export const slackAdapter: ChannelAdapter = {
  id: "slack",

  async verify(req, env, now) {
    const secret = env.SLACK_SIGNING_SECRET;
    const signature = req.headers.get("x-slack-signature");
    const timestamp = req.headers.get("x-slack-request-timestamp");
    if (!secret || !signature || !timestamp) return false;
    if (Math.abs(now - Number(timestamp)) > SIGNATURE_MAX_AGE_SECONDS) return false;

    // Cloned so the original body is still readable afterwards -- `router.ts`'s webhook handler
    // calls `verify` before it ever parses the body itself, and a `Request`'s body can only be
    // read once.
    const raw = await req.clone().text();
    const expected = `v0=${await hmacSha256Hex(secret, `v0:${timestamp}:${raw}`)}`;
    return timingSafeEqual(signature, expected);
  },

  routingKey(payload) {
    const callback = asEventCallback(payload);
    return callback ? callback.team_id : null;
  },

  challengeResponse(payload) {
    if (typeof payload !== "object" || payload === null) return null;
    const envelope = payload as SlackUrlVerification;
    return envelope.type === "url_verification" && typeof envelope.challenge === "string"
      ? envelope.challenge
      : null;
  },

  async normalize(payload, connection) {
    const callback = asEventCallback(payload);
    if (!callback) return [];
    const { event } = callback;

    if (event.type === "assistant_thread_started") {
      const thread = (event as SlackAssistantThreadEvent).assistant_thread;
      return [
        {
          v: 1,
          id: `slack:${callback.event_id}`,
          provider: "slack",
          connection,
          thread: threadKey(thread.channel_id, thread.thread_ts),
          sender: { id: thread.user_id, name: thread.user_id },
          text: "",
          attachments: [],
          raw_kind: event.type,
          received_at: Date.now(),
        },
      ];
    }

    // Every other event type this adapter handles shares `SlackEventBase`'s shape --
    // `SlackAssistantThreadEvent` is the one exception, already returned above.
    const base = event as SlackEventBase;
    const isMessageIm = base.type === "message" && base.channel_type === "im";
    const isAppMention = base.type === "app_mention";
    if ((!isMessageIm && !isAppMention) || !isOrdinaryUserEvent(base) || !base.channel || !base.user) {
      return [];
    }

    const attachments: ChannelMessage["attachments"] = (base.files ?? [])
      .filter((f): f is { url_private: string; name?: string; mimetype?: string } => Boolean(f.url_private))
      .map((f) => ({
        kind: f.mimetype?.startsWith("image/") ? "image" : "file",
        url: f.url_private,
        name: f.name,
      }));

    return [
      {
        v: 1,
        id: `slack:${callback.event_id}`,
        provider: "slack",
        connection,
        thread: threadKey(base.channel, base.thread_ts),
        sender: { id: base.user, name: base.user },
        text: base.text ?? "",
        attachments,
        raw_kind: base.type,
        received_at: Date.now(),
      },
    ];
  },

  async send(reply: OutboundReply, credential: unknown) {
    const { botToken } = credential as SlackCredential;
    if (!reply.thread) {
      // Every connection's thread map is established at link time (§5); a reply with nothing to
      // address isn't a case `/send`'s caller should be able to reach, so this fails loudly
      // rather than guessing a channel from the workspace-wide routing key.
      throw new Error("slack send needs a channel (OutboundReply.thread), not just the workspace routing key");
    }
    const [channel, threadTs] = splitThread(reply.thread);
    const res = await fetch("https://slack.com/api/chat.postMessage", {
      method: "POST",
      headers: { "content-type": "application/json; charset=utf-8", authorization: `Bearer ${botToken}` },
      body: JSON.stringify({ channel, text: reply.text, ...(threadTs ? { thread_ts: threadTs } : {}) }),
    });
    const data = (await res.json()) as { ok: boolean; error?: string };
    if (!res.ok || !data.ok) {
      throw new Error(`slack chat.postMessage failed: ${data.error ?? res.status}`);
    }
  },

  async setThinking(thread: string, credential: unknown, status: string) {
    const { botToken } = credential as SlackCredential;
    const [channel, threadTs] = splitThread(thread);
    if (!threadTs) return; // setStatus is a per-thread indicator; a bare channel has none to set
    const res = await fetch("https://slack.com/api/assistant.threads.setStatus", {
      method: "POST",
      headers: { "content-type": "application/json; charset=utf-8", authorization: `Bearer ${botToken}` },
      body: JSON.stringify({ channel_id: channel, thread_ts: threadTs, status }),
    });
    const data = (await res.json()) as { ok: boolean; error?: string };
    if (!res.ok || !data.ok) {
      throw new Error(`slack assistant.threads.setStatus failed: ${data.error ?? res.status}`);
    }
  },

  async resolveSenderName(senderId: string, credential: unknown): Promise<string | null> {
    const { botToken } = credential as SlackCredential;
    const cacheKey = `${botToken}:${senderId}`;
    const cached = senderNameCache.get(cacheKey);
    if (cached && cached.expiresAt > Date.now()) return cached.name;

    const name = await usersInfo(botToken, senderId);
    if (name) senderNameCache.set(cacheKey, { name, expiresAt: Date.now() + SENDER_NAME_CACHE_TTL_MS });
    return name;
  },
};

/** The one network call `resolveSenderName` makes -- `users.info`, the display name a human
 * actually goes by (`profile.display_name`, falling back to `profile.real_name` for a workspace
 * member who never set one) rather than the raw id `normalize` had to settle for on its own. */
async function usersInfo(botToken: string, userId: string): Promise<string | null> {
  const res = await fetch(`https://slack.com/api/users.info?user=${encodeURIComponent(userId)}`, {
    headers: { authorization: `Bearer ${botToken}` },
  });
  const data = (await res.json()) as {
    ok: boolean;
    user?: { profile?: { display_name?: string; real_name?: string } };
  };
  if (!res.ok || !data.ok) return null;
  const name = data.user?.profile?.display_name || data.user?.profile?.real_name;
  return name && name.length > 0 ? name : null;
}

/** The inverse of `threadKey` -- a channel, or a `channel:thread_ts` pair. */
function splitThread(thread: string): [string, string | undefined] {
  const colon = thread.indexOf(":");
  return colon < 0 ? [thread, undefined] : [thread.slice(0, colon), thread.slice(colon + 1)];
}

/** "Add to Slack" -- the OAuth v2 authorize URL, state carrying the signed link code exactly the
 * way `oauth-router`'s own `state` param does (docs/channels.md §4, "install/link"). `null`
 * without `SLACK_CLIENT_ID` configured, the same convention `telegramInstallUrl` uses. */
export function slackInstallUrl(requestUrl: string, env: Env, code: string): string | null {
  if (!env.SLACK_CLIENT_ID) return null;
  const url = new URL("https://slack.com/oauth/v2/authorize");
  url.searchParams.set("client_id", env.SLACK_CLIENT_ID);
  url.searchParams.set("scope", SLACK_BOT_SCOPES.join(","));
  url.searchParams.set("redirect_uri", slackRedirectUri(requestUrl));
  url.searchParams.set("state", code);
  return url.toString();
}

/** `GET /link/slack`, on this deployment's own origin -- the one URL registered with Slack as
 * this app's OAuth redirect. */
export function slackRedirectUri(requestUrl: string): string {
  return new URL("/link/slack", requestUrl).toString();
}

export type SlackExchangeResult =
  | { ok: true; teamId: string; botToken: string }
  | { ok: false; error: string };

/** Exchange the OAuth authorization code for this workspace's bot token, server-to-server --
 * the callback leg of "Add to Slack" (docs/channels.md §4). */
export async function slackExchangeCode(
  env: Env,
  code: string,
  redirectUri: string,
): Promise<SlackExchangeResult> {
  if (!env.SLACK_CLIENT_ID || !env.SLACK_CLIENT_SECRET) {
    return { ok: false, error: "slack is not configured on this deployment" };
  }
  const res = await fetch("https://slack.com/api/oauth.v2.access", {
    method: "POST",
    headers: { "content-type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      client_id: env.SLACK_CLIENT_ID,
      client_secret: env.SLACK_CLIENT_SECRET,
      code,
      redirect_uri: redirectUri,
    }),
  });
  const data = (await res.json()) as {
    ok: boolean;
    error?: string;
    access_token?: string;
    team?: { id?: string };
  };
  if (!res.ok || !data.ok) return { ok: false, error: data.error ?? `http ${res.status}` };
  if (!data.access_token || !data.team?.id) {
    return { ok: false, error: "oauth.v2.access response missing access_token or team.id" };
  }
  return { ok: true, teamId: data.team.id, botToken: data.access_token };
}

/** What a browser sees once "Add to Slack" completes -- there is no chat yet to send a welcome
 * message into (unlike Telegram's `/start`, Slack's workspace-level routing key names no
 * channel), so the tab itself carries the confirmation instead. */
export const SLACK_INSTALL_SUCCESS_HTML =
  "<!doctype html><title>Connected to ADI</title><p>Linked. You can close this tab and go back to Slack.</p>";
