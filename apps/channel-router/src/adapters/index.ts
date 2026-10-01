/** Adapter registry. `slack` is a provider-registry entry (`providers.ts`) with no adapter
 * here yet -- `getAdapter("slack")` returns `undefined` until ADI-MONO-123. */
import type { ChannelAdapter } from "../types";
import { telegramAdapter } from "./telegram";

const ADAPTERS: Record<string, ChannelAdapter> = {
  telegram: telegramAdapter,
};

export function getAdapter(providerId: string): ChannelAdapter | undefined {
  return ADAPTERS[providerId];
}

/** Resolve the provider-specific credential an adapter's `send` needs. Each provider's shape
 * differs (§4), so this is a small per-provider switch rather than a uniform lookup. */
export function resolveCredential(providerId: string, env: { TELEGRAM_BOT_TOKEN?: string }): unknown {
  if (providerId === "telegram") {
    return env.TELEGRAM_BOT_TOKEN ? { botToken: env.TELEGRAM_BOT_TOKEN } : null;
  }
  return null;
}
