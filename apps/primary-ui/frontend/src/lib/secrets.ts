import { settingsRequest, type SettingsProject } from "./settings-api";

/*
 * Secrets: encrypted key-values a machine injects into runs as environment variables. A secret
 * lives in one scope — global, or one project (where it overrides a global of the same name).
 *
 * The list is metadata only: the native API never returns a value with it. A value leaves the
 * machine through one deliberate request per secret (`/secrets/reveal`), and this module never
 * puts one in a cache — the page holds a revealed value in its own state and drops it again.
 */

export type SecretOAuth = { provider: string; obtained_at?: number; expires_at?: number | null; scope?: string | null; has_refresh?: boolean };
export type Secret = { project?: string | null; name: string; description?: string | null; created_at: number; updated_at: number; oauth?: SecretOAuth | null };
export type SecretsState = { secrets: Secret[] };
export type SecretRef = { name: string; project: string | null };
/** `POST /secrets/set` — create or overwrite. An omitted description is stored as none. */
export type SetSecret = SecretRef & { value: string; description: string | null };
export type SetOAuthSecret = SecretRef & {
  description: string | null; provider: string; access_token: string; refresh_token: string | null; expires_in: number | null; scope: string | null;
};
export type RevealedSecret = SecretRef & { value: string };

export const secretsKey = (source?: string) => ["settings-secrets", source ?? ""] as const;
export const loadSecrets = (source?: string) => ({ signal }: { signal: AbortSignal }) => settingsRequest<SecretsState>(source, "/secrets", undefined, signal);

/** A secret is its scope and its name; a global and a project secret of one name are two secrets. */
export const secretId = (secret: { project?: string | null; name: string }) => `${secret.project ?? ""}\u0001${secret.name}`;
export const secretRef = (secret: { project?: string | null; name: string }): SecretRef => ({ name: secret.name, project: secret.project || null });

/** A secret reaches a run as an environment variable, so its name is an identifier. */
export const secretNamePattern = /^[A-Za-z_][A-Za-z0-9_]*$/;
export function secretNameProblem(name: string): string | null {
  const trimmed = name.trim();
  if (!trimmed) return "A secret name is required.";
  if (!secretNamePattern.test(trimmed)) return "Use letters, digits and underscores, not starting with a digit: the name is the environment variable.";
  return null;
}

/** Where a secret's views live; the name and scope travel in the query. */
export function secretHref(secret?: { project?: string | null; name: string }): string {
  if (!secret) return "/settings/secrets";
  return `/settings/secrets?${new URLSearchParams({ secret: secret.name, ...(secret.project ? { project: secret.project } : {}) })}`;
}
export const newSecretHref = (project?: string | null) => `/settings/secrets?${new URLSearchParams({ new: "1", ...(project ? { project } : {}) })}`;

/** Global first, then project by project, each by name. */
export function sortSecrets(secrets: Secret[]): Secret[] {
  return [...secrets].sort((a, b) => (a.project ?? "").localeCompare(b.project ?? "") || a.name.localeCompare(b.name));
}

/** The scope filter's value: every scope, the global one, or one project's id. */
export type ScopeFilter = "all" | "global" | { project: string };
export const scopeFilterValue = (filter: ScopeFilter) => (typeof filter === "string" ? filter : `project:${filter.project}`);
export function parseScopeFilter(value: string): ScopeFilter {
  if (value === "global") return "global";
  return value.startsWith("project:") && value.length > 8 ? { project: value.slice(8) } : "all";
}

export function projectName(project: string, projects: SettingsProject[] = []): string {
  return projects.find((item) => item.id === project)?.name ?? project;
}

/** The scopes that hold secrets, with how many each holds: global first, then projects by name. */
export function secretScopes(secrets: Secret[], projects: SettingsProject[] = []): { value: string; label: string; count: number }[] {
  const counts = new Map<string, number>();
  for (const secret of secrets) if (secret.project) counts.set(secret.project, (counts.get(secret.project) ?? 0) + 1);
  const global = secrets.length - [...counts.values()].reduce((sum, count) => sum + count, 0);
  return [
    { value: "all", label: "All scopes", count: secrets.length },
    { value: "global", label: "Global", count: global },
    ...[...counts].map(([project, count]) => ({ value: `project:${project}`, label: projectName(project, projects), count }))
      .sort((a, b) => a.label.localeCompare(b.label)),
  ];
}

/** The list's filter: a scope, and every search word against name, description, project and provider. */
export function filterSecrets(secrets: Secret[], query: string, scope: ScopeFilter = "all", projects: SettingsProject[] = []): Secret[] {
  const words = query.trim().toLowerCase().split(/\s+/).filter(Boolean);
  return secrets.filter((secret) => {
    if (scope === "global" ? !!secret.project : scope !== "all" && secret.project !== scope.project) return false;
    if (!words.length) return true;
    const text = [secret.name, secret.description, secret.project, secret.project ? projectName(secret.project, projects) : "global", secret.oauth?.provider].join(" ").toLowerCase();
    return words.every((word) => text.includes(word));
  });
}

// ---- OAuth-sourced secrets ----------------------------------------------------------------

export function providerLabel(provider: string): string {
  if (provider === "google") return "Google";
  if (provider === "github") return "GitHub";
  return provider ? provider[0].toUpperCase() + provider.slice(1) : "OAuth";
}

/** A coarse "expires in …" / "expired" from an absolute expiry, or `null` when the provider gave none. */
export function expiryText(expiresAt: number | null | undefined, now = Date.now()): string | null {
  if (expiresAt == null) return null;
  const seconds = expiresAt - Math.floor(now / 1000);
  if (seconds <= 0) return "expired";
  const minutes = Math.floor(seconds / 60);
  if (minutes < 60) return `expires in ${minutes}m`;
  if (minutes < 48 * 60) return `expires in ${Math.floor(minutes / 60)}h`;
  return `expires in ${Math.floor(minutes / 1440)}d`;
}

export const oauthProviders = [{ value: "google", label: "Google" }, { value: "github", label: "GitHub" }] as const;

/**
 * The access a provider can be asked for, as scope and label. These are requested; the provider
 * answers with what it granted, and that is what is stored. A provider without a list is asked
 * for its default access.
 */
export function providerAccesses(provider: string): readonly { scope: string; label: string }[] {
  if (provider !== "google") return [];
  return [
    { scope: "https://www.googleapis.com/auth/gmail.readonly", label: "Gmail — read (messages, threads, labels)" },
    { scope: "https://www.googleapis.com/auth/gmail.send", label: "Gmail — send email" },
    { scope: "https://www.googleapis.com/auth/gmail.modify", label: "Gmail — read, send and manage (labels, drafts, archive; no permanent delete)" },
    { scope: "https://mail.google.com/", label: "Gmail — full access (including permanent delete)" },
    { scope: "email", label: "Account email address" },
  ];
}

/**
 * The router that runs a provider's sign-in and hands the token back in the redirect fragment.
 * Off this machine by necessity: a provider will not redirect to a private `.adi` name.
 */
export const oauthRouter = (import.meta.env?.VITE_OAUTH_ROUTER_URL as string | undefined)?.replace(/\/+$/, "") || "https://oauth-router.withadi.dev";

/** The sign-in address for a provider, returning to `returnTo` with the token in its fragment. */
export function oauthLoginUrl(provider: string, returnTo: string, scope?: string | null, router = oauthRouter): string {
  const query = new URLSearchParams({ redirect: returnTo, ...(scope?.trim() ? { scope: scope.trim() } : {}) });
  return `${router}/login/${encodeURIComponent(provider)}?${query}`;
}

/** The secret a sign-in is for, parked across the provider round-trip. */
export type PendingOAuth = SecretRef & { description: string | null; provider: string; /** The machine the secret is stored on. */ source?: string };

/**
 * Parked in localStorage, not sessionStorage: the sign-in runs in a new tab, a separate browsing
 * context, and has to find the intent there. The old panel parks its own under the same key.
 */
export const pendingOAuthKey = "adi.oauth.pending";
type Storage = Pick<globalThis.Storage, "getItem" | "setItem" | "removeItem">;

export function parkOAuth(pending: PendingOAuth, storage: Storage = localStorage) {
  storage.setItem(pendingOAuthKey, JSON.stringify(pending));
}

/** Read and remove the parked intent — one shot, so a reload of the return address cannot store twice. */
export function takeOAuth(storage: Storage = localStorage): PendingOAuth | null {
  try {
    const raw = storage.getItem(pendingOAuthKey);
    storage.removeItem(pendingOAuthKey);
    const value = JSON.parse(raw ?? "null");
    if (!value || typeof value.name !== "string" || !secretNamePattern.test(value.name) || typeof value.provider !== "string" || !value.provider) return null;
    const source = typeof value.source === "string" && /^[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?$/.test(value.source) ? value.source : undefined;
    return {
      name: value.name, project: typeof value.project === "string" && value.project ? value.project : null,
      description: typeof value.description === "string" && value.description.trim() ? value.description : null,
      provider: value.provider, ...(source ? { source } : {}),
    };
  } catch { return null; }
}

export type OAuthReturn =
  | { kind: "none" }
  | { kind: "error"; message: string }
  | { kind: "token"; accessToken: string; refreshToken: string | null; expiresIn: number | null; scope: string | null; provider: string | null };

/** What a location fragment says about a returning sign-in. Anything else on the page is `none`. */
export function parseOAuthReturn(hash: string): OAuthReturn {
  const fragment = hash.replace(/^#/, "");
  if (!fragment) return { kind: "none" };
  const params = new URLSearchParams(fragment);
  const error = params.get("error");
  if (error) return { kind: "error", message: params.get("error_description") || error };
  const accessToken = params.get("access_token");
  if (!accessToken) return { kind: "none" };
  const expiresIn = Number(params.get("expires_in"));
  return {
    kind: "token", accessToken, refreshToken: params.get("refresh_token") || null,
    expiresIn: Number.isFinite(expiresIn) && expiresIn > 0 ? Math.floor(expiresIn) : null,
    scope: params.get("scope") || null, provider: params.get("provider") || null,
  };
}

/** The store request for a token that came back for a parked intent. */
export function oauthSecretBody(pending: PendingOAuth, token: Extract<OAuthReturn, { kind: "token" }>): SetOAuthSecret {
  return {
    name: pending.name, project: pending.project, description: pending.description, provider: token.provider ?? pending.provider,
    access_token: token.accessToken, refresh_token: token.refreshToken, expires_in: token.expiresIn, scope: token.scope,
  };
}
