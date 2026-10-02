/** Adapter registry. */
import type { ChannelAdapter, Connection, Env } from "../types";
import { decryptCredential } from "../state";
import { slackAdapter } from "./slack";
import { telegramAdapter } from "./telegram";

const ADAPTERS: Record<string, ChannelAdapter> = {
  telegram: telegramAdapter,
  slack: slackAdapter,
};

export function getAdapter(providerId: string): ChannelAdapter | undefined {
  return ADAPTERS[providerId];
}

/** Resolve the provider-specific credential an adapter's `send`/`setThinking` needs. Each
 * provider's shape -- and where it lives -- differs (§4): Telegram's one bot token is a Worker
 * secret, looked up from `env` alone; Slack's is per-workspace, encrypted inside the connection
 * its own Durable Object holds (`connection.credential`), so resolving it needs that connection
 * and `ROUTER_SECRET` to decrypt with. `connection` is `null`/absent wherever no connection is in
 * hand yet (the Telegram-only link-attempt path in `router.ts`, which never resolves Slack's). */
export async function resolveCredential(
  providerId: string,
  env: Env,
  connection?: Connection | null,
): Promise<unknown> {
  if (providerId === "telegram") {
    return env.TELEGRAM_BOT_TOKEN ? { botToken: env.TELEGRAM_BOT_TOKEN } : null;
  }
  if (providerId === "slack") {
    if (!connection?.credential) return null;
    return decryptCredential(connection.credential, env.ROUTER_SECRET);
  }
  return null;
}
