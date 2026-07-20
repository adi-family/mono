/**
 * The generic provider registry. Adding an OAuth provider is adding a {@link ProviderDef}
 * entry to {@link PROVIDERS} and setting its `<ID>_CLIENT_ID` / `<ID>_CLIENT_SECRET` on the
 * deployment — no router code changes. The registry holds only *public* facts (the two
 * well-known endpoints and sensible default scopes); credentials live in the environment.
 */

import type { Env } from "./types";

/** The public, deployment-independent shape of one OAuth provider. */
export interface ProviderDef {
  /** Registry key, also the `/login/<id>` and `/callback/<id>` path segment. */
  id: string;
  /** Authorization endpoint the browser is redirected to. */
  authUrl: string;
  /** Token endpoint the code is exchanged at, server-to-server. */
  tokenUrl: string;
  /** Scopes requested when `_SCOPES` / `?scope=` don't override them. */
  defaultScopes: string[];
  /** Extra static params appended to the authorize URL (e.g. Google's `access_type`). */
  authParams?: Record<string, string>;
  /** How scopes are joined in the authorize URL. Default `" "`. */
  scopeSeparator?: string;
}

/**
 * Built-in providers. These are just the well-known endpoints and sane default scopes;
 * per-deployment credentials come from {@link Env}. Add more by copying a block.
 */
export const PROVIDERS: Record<string, ProviderDef> = {
  google: {
    id: "google",
    authUrl: "https://accounts.google.com/o/oauth2/v2/auth",
    tokenUrl: "https://oauth2.googleapis.com/token",
    defaultScopes: ["openid", "email", "profile"],
    // access_type=offline + prompt=consent are what make Google return a refresh_token.
    authParams: { access_type: "offline", prompt: "consent" },
  },
  github: {
    id: "github",
    authUrl: "https://github.com/login/oauth/authorize",
    tokenUrl: "https://github.com/login/oauth/access_token",
    defaultScopes: ["read:user", "user:email"],
  },
};

/** A provider resolved against a deployment: the public def plus its live credentials. */
export interface ResolvedProvider extends ProviderDef {
  clientId: string;
  clientSecret: string;
  scopes: string[];
}

/** Split a scope string on whitespace or commas, dropping empties. */
function parseScopes(raw: string): string[] {
  return raw.split(/[\s,]+/).filter(Boolean);
}

/**
 * Resolve a provider id against the environment. Returns `null` when the id is unknown or
 * when the deployment hasn't configured its client id/secret — both read to the caller as
 * "this provider isn't available here", which is exactly the right thing to 404 on.
 */
export function resolveProvider(id: string, env: Env): ResolvedProvider | null {
  const def = PROVIDERS[id];
  if (!def) return null;

  const key = id.toUpperCase();
  const clientId = env[`${key}_CLIENT_ID`];
  const clientSecret = env[`${key}_CLIENT_SECRET`];
  if (!clientId || !clientSecret) return null;

  const override = env[`${key}_SCOPES`];
  const scopes = override ? parseScopes(override) : def.defaultScopes;
  return { ...def, clientId, clientSecret, scopes };
}

/** The provider ids that are actually enabled on this deployment (creds present). */
export function enabledProviders(env: Env): string[] {
  return Object.keys(PROVIDERS).filter((id) => resolveProvider(id, env) !== null);
}
