/**
 * The provider registry -- public facts only, exactly oauth-router's `providers.ts` convention
 * ("public facts in the registry, credentials from env keyed on the id"). Unlike oauth-router's
 * providers, a channel provider's credential shape isn't uniform (Telegram: one static bot
 * token; Slack: a token per workspace, minted at OAuth time and stored in the connection's own
 * Durable Object, never a Worker secret -- docs/channels.md §4), so there is no single
 * `resolveProvider` returning a common shape here; each adapter reads its own env fields.
 *
 * `telegram` and, since ADI-MONO-123, `slack` both have working adapters (see
 * `adapters/index.ts`) -- each reports enabled once its own credentials are set, independently.
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
    enabled: (env) => Boolean(env.SLACK_CLIENT_ID && env.SLACK_CLIENT_SECRET && env.SLACK_SIGNING_SECRET),
  },
};

/** The provider ids actually enabled on this deployment -- what `GET /health` reports. */
export function enabledProviders(env: Env): string[] {
  return Object.values(PROVIDERS)
    .filter((p) => p.enabled(env))
    .map((p) => p.id);
}
