/**
 * The Telegram adapter -- docs/channels.md §4, "Telegram, worked through it".
 *
 * - `verify`: Telegram has no HMAC; it has a secret path segment instead, echoed back as
 *   `X-Telegram-Bot-Api-Secret-Token`.
 * - `routingKey`: `message.chat.id` -- a DM, group, or channel (Telegram calls all three
 *   "chat").
 * - `normalize`: one `ChannelMessage` per update. A photo's largest `PhotoSize.file_id` is
 *   resolved through `getFile` to a URL before forwarding -- Telegram file URLs expire, so this
 *   has to happen now, not whenever the node eventually fetches it.
 * - `extractLinkCode`: Telegram's own "link" is just another webhook -- `/start <code>` arrives
 *   as an ordinary `message` update, so there's no separate `GET /link/telegram` route; the
 *   webhook handler asks every adapter this instead.
 * - `send`: `POST .../sendMessage` with the one bot token this deployment holds.
 */
import type { ChannelAdapter, ChannelMessage, Env, OutboundReply } from "../types";
import { timingSafeEqual } from "../http";

export interface TelegramCredential {
  botToken: string;
}

interface TelegramUser {
  id: number;
  username?: string;
  first_name?: string;
}

interface TelegramChat {
  id: number;
}

interface TelegramPhotoSize {
  file_id: string;
  width: number;
  height: number;
}

interface TelegramMessage {
  message_id: number;
  message_thread_id?: number;
  chat: TelegramChat;
  from?: TelegramUser;
  text?: string;
  caption?: string;
  photo?: TelegramPhotoSize[];
  reply_to_message?: { message_id: number };
}

interface TelegramUpdate {
  update_id: number;
  message?: TelegramMessage;
  edited_message?: TelegramMessage;
  channel_post?: TelegramMessage;
}

function messageOf(update: TelegramUpdate): { message: TelegramMessage; rawKind: string } | null {
  if (update.message) return { message: update.message, rawKind: "message" };
  if (update.edited_message) return { message: update.edited_message, rawKind: "edited_message" };
  if (update.channel_post) return { message: update.channel_post, rawKind: "channel_post" };
  return null;
}

function startCommandArg(text: string | undefined): string | null {
  if (!text) return null;
  const match = /^\/start(?:@\w+)?(?:\s+(\S+))?/.exec(text.trim());
  if (!match) return null;
  return match[1] ?? "";
}

/** Resolve a photo's largest size to a fetchable URL via `getFile`, the one network round-trip
 * `normalize` makes -- Telegram's file URLs are short-lived, so this has to happen before
 * forwarding, never deferred to whenever the node gets around to it. */
async function resolvePhotoUrl(botToken: string, photo: TelegramPhotoSize[]): Promise<string | null> {
  const largest = photo.reduce((a, b) => (b.width * b.height > a.width * a.height ? b : a));
  const res = await fetch(`https://api.telegram.org/bot${botToken}/getFile?file_id=${largest.file_id}`);
  if (!res.ok) return null;
  const body = (await res.json()) as { ok: boolean; result?: { file_path?: string } };
  if (!body.ok || !body.result?.file_path) return null;
  return `https://api.telegram.org/file/bot${botToken}/${body.result.file_path}`;
}

export const telegramAdapter: ChannelAdapter = {
  id: "telegram",

  async verify(req, env) {
    const header = req.headers.get("x-telegram-bot-api-secret-token");
    const secret = env.TELEGRAM_SECRET_TOKEN;
    if (!header || !secret) return false;
    return timingSafeEqual(header, secret);
  },

  routingKey(payload) {
    const found = messageOf(payload as TelegramUpdate);
    return found ? String(found.message.chat.id) : null;
  },

  extractLinkCode(payload) {
    const found = messageOf(payload as TelegramUpdate);
    if (!found) return null;
    return startCommandArg(found.message.text);
  },

  async normalize(payload, connection, env) {
    const update = payload as TelegramUpdate;
    const found = messageOf(update);
    if (!found) return [];
    const { message, rawKind } = found;

    // A /start is a link attempt, handled by the webhook route before normalize is ever
    // called on an ordinary message -- but guard here too, since normalize is a pure function
    // of its inputs and shouldn't assume the caller always checked first.
    if (startCommandArg(message.text) !== null) return [];

    const attachments: ChannelMessage["attachments"] = [];
    if (message.photo && message.photo.length > 0 && env.TELEGRAM_BOT_TOKEN) {
      const url = await resolvePhotoUrl(env.TELEGRAM_BOT_TOKEN, message.photo);
      if (url) attachments.push({ kind: "image", url });
    }

    const thread =
      message.message_thread_id != null
        ? `${message.chat.id}:${message.message_thread_id}`
        : String(message.chat.id);

    return [
      {
        v: 1,
        id: `telegram:${update.update_id}`,
        provider: "telegram",
        connection,
        thread,
        sender: {
          id: String(message.from?.id ?? message.chat.id),
          name: message.from?.username ?? message.from?.first_name ?? "unknown",
        },
        text: message.text ?? message.caption ?? "",
        attachments,
        reply_to: message.reply_to_message ? `telegram:${message.reply_to_message.message_id}` : undefined,
        raw_kind: rawKind,
        received_at: Date.now(),
      },
    ];
  },

  async send(reply: OutboundReply, credential: unknown) {
    const { botToken } = credential as TelegramCredential;
    const res = await fetch(`https://api.telegram.org/bot${botToken}/sendMessage`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ chat_id: reply.routingKey, text: reply.text }),
    });
    if (!res.ok) {
      throw new Error(`telegram sendMessage failed: http ${res.status}`);
    }
  },
};

/** Where `t.me/<bot>?start=<code>` points an operator at, for the welcome message §1 step 3
 * describes the webhook handler sending itself. */
export function telegramInstallUrl(env: Env, code: string): string | null {
  if (!env.TELEGRAM_BOT_USERNAME) return null;
  return `https://t.me/${env.TELEGRAM_BOT_USERNAME}?start=${code}`;
}
