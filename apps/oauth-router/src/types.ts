/**
 * The Worker's runtime configuration, injected by Cloudflare from `wrangler.toml`
 * `[vars]` (public) and `wrangler secret put` (secret). The code itself carries no
 * credentials — everything the router needs to run is here.
 *
 * Per-provider credentials follow a fixed convention keyed by the uppercased provider
 * id (see {@link ./providers.ts}):
 *
 * * `<PROVIDER>_CLIENT_ID`     — required to enable the provider on this deployment
 * * `<PROVIDER>_CLIENT_SECRET` — required
 * * `<PROVIDER>_SCOPES`        — optional, overrides the built-in default scopes
 *
 * A provider whose `_CLIENT_ID` is unset is simply "not enabled" here — `/login/<it>`
 * 404s rather than leaking a misconfigured flow.
 */
export interface Env {
  /**
   * HMAC key that signs the opaque `state`. A tampered or forged callback fails
   * verification, so only flows this Worker actually started can complete.
   */
  STATE_SECRET: string;

  /**
   * Default token-delivery target — where the exchanged token is handed back when a
   * `/login` request carries no explicit `?redirect=`. Its origin is always allow-listed.
   */
  APP_URL: string;

  /**
   * Comma-separated extra origins a caller-supplied `?redirect=` may target, on top of
   * `APP_URL`'s origin. This is the open-redirect / token-exfiltration guard: a redirect
   * to an origin not on the list is refused. Unset ⇒ only `APP_URL`'s origin is allowed.
   */
  ALLOWED_REDIRECT_ORIGINS?: string;

  /**
   * When exactly `"true"`, a provider's `refresh_token` (if returned) is forwarded to the
   * app alongside the access token. Off by default — refresh tokens are long-lived and
   * shouldn't reach a browser unless the app is built to hold them.
   */
  INCLUDE_REFRESH_TOKEN?: string;

  /** Per-provider credentials and scope overrides, read dynamically by convention. */
  [key: string]: string | undefined;
}
