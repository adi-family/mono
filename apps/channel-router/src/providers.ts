/**
 * The provider registry -- public facts only, exactly oauth-router's `providers.ts` convention
 * ("public facts in the registry, credentials from env keyed on the id"). Unlike oauth-router's
 * providers, a channel provider's credential shape isn't uniform (Telegram: one static bot
 * token; Slack: a token per workspace, minted at OAuth time and stored in the connection's own
 * Durable Object, never a Worker secret -- docs/channels.md §4), so there is no single
 * `resolveProvider` returning a common shape here; each adapter reads its own env fields.
 *
 * `telegram` is the only provider with a working adapter (ADI-MONO-120's scope). `slack` is
 * named here only so `enabledProviders`/`GET /health` already report the full roster shape
 * `docs/channels.md` §6 expects to grow into -- it has no adapter, so every route that needs
 * one 404s for it today (see `adapters/index.ts`).
 */
import type { Env } from "./types";

export interface ProviderDef {
  id: string;
  name: string;
  /** Whether this deployment has what the provider needs configured. Always `false` for a
   * provider with no adapter yet (slack), regardless of env. */
  enabled(env: Env): boolean;
}

export const PROVIDERS: Record<string, ProviderDef> = {
  telegram: {
    id: "telegram",
    name: "Telegram",
    enabled: (env) => Boolean(env.TELEGRAM_BOT_TOKEN && env.TELEGRAM_SECRET_TOKEN),
  },
  slack: {
    id: "slack",
    name: "Slack",
    // Stub: no adapter exists yet (ADI-MONO-123), so never enabled regardless of env.
    enabled: () => false,
  },
};

/** The provider ids actually enabled on this deployment -- what `GET /health` reports. */
export function enabledProviders(env: Env): string[] {
  return Object.values(PROVIDERS)
    .filter((p) => p.enabled(env))
    .map((p) => p.id);
}
