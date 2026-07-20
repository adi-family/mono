# adi-oauth-router

A generic **OAuth router**, deployed as a Cloudflare Worker on **`oauth-router.withadi.dev`**. It fronts many OAuth providers
(Google, GitHub, …) behind two routes, and its whole job is to take the token a provider
hands back and **route it to the app** — by redirecting the browser back to the app with the
token in the URL fragment.

```
  browser ──/login/google──▶  router ──302──▶  accounts.google.com
                                                      │  (user approves)
  app  ◀──302 #access_token=…──  router  ◀──/callback/google?code=…──┘
                                   └─ exchanges code for token server-to-server
```

The Worker holds **no session and no database**. What the callback needs to know — which
provider, where to send the token, a CSRF nonce — is carried across the round-trip in a
signed, expiring `state`, verified on the way back.

## Routes

| Route | What it does |
| --- | --- |
| `GET /` · `GET /health` | JSON: the service name and which providers are configured. |
| `GET /login/<provider>?redirect=<app-url>&scope=<optional>` | Redirects to the provider's consent screen. `redirect` (optional) is where the token is delivered; it must be allow-listed. `scope` (optional) overrides the provider's default scopes. |
| `GET /callback/<provider>?code=…&state=…` | The URL you register with the provider. Exchanges the code and redirects to the app with the token in the fragment. |
| `POST /refresh/<provider>` `{ refresh_token }` | Mints a fresh access token from a stored refresh token, server-to-server (JSON in, JSON out). Lets a saved token renew without the user re-authorizing. The refresh token is the credential, so no extra auth is imposed; the client secret stays in the Worker. |

### What the app receives

On success the app is redirected to `<app-url>#access_token=…&token_type=…&expires_in=…&scope=…&provider=…`
(plus `id_token` for OIDC providers like Google). Read it in the app from `location.hash`:

```js
const t = new URLSearchParams(location.hash.slice(1));
t.get("access_token"); t.get("provider"); t.get("error");
```

On failure (user denied, or the exchange failed) the fragment carries `error` and
`error_description` instead. The token rides the **fragment**, never the query string, so it
never reaches a server log or a `Referer` header.

## Providers

The registry is [`src/providers.ts`](src/providers.ts). Each provider is a small block of
**public** facts (authorize URL, token URL, default scopes). Adding one is copying a block —
no other code changes. Credentials are never in the registry; they come from the environment
by convention, keyed on the uppercased id:

- `GOOGLE_CLIENT_ID`, `GOOGLE_CLIENT_SECRET`, optional `GOOGLE_SCOPES`
- `GITHUB_CLIENT_ID`, `GITHUB_CLIENT_SECRET`, optional `GITHUB_SCOPES`

A provider with no client id set is simply not enabled — its `/login` 404s.

## Configuration

| Name | Kind | Purpose |
| --- | --- | --- |
| `STATE_SECRET` | secret | HMAC key that signs `state`. `openssl rand -hex 32`. |
| `APP_URL` | var | Default token-delivery target; its origin is always allow-listed. |
| `ALLOWED_REDIRECT_ORIGINS` | var | Extra comma-separated origins a `?redirect=` may target. |
| `INCLUDE_REFRESH_TOKEN` | var | `"true"` to also forward the provider's `refresh_token`. |
| `<PROVIDER>_CLIENT_ID` / `_CLIENT_SECRET` / `_SCOPES` | secret / var | Per-provider credentials and scope override. |

Vars live in [`wrangler.toml`](wrangler.toml) under `[vars]`; secrets are set with
`wrangler secret put <NAME>` and never committed.

## Develop & deploy

```bash
cd apps/oauth-router
bun install                 # or: npm install

cp .dev.vars.example .dev.vars   # fill in local secrets (git-ignored)
bun run dev                 # wrangler dev — local server

bun run typecheck           # tsc --noEmit
bun run test                # vitest

# Register each provider's redirect URI as https://oauth-router.withadi.dev/callback/<provider>,
# set the secrets, then:
bun run deploy              # wrangler deploy
```

The Worker is bound to **`oauth-router.withadi.dev`** via the `routes` entry in
[`wrangler.toml`](wrangler.toml) (`custom_domain = true`). This requires the `withadi.dev`
zone to be on the same Cloudflare account; `wrangler deploy` then provisions the DNS record
and TLS certificate for `oauth-router.withadi.dev` automatically. Redirect URIs registered at each
provider must therefore be `https://oauth-router.withadi.dev/callback/<provider>`.

### Deploying as a Cloudflare Pages project instead

The same router is wired through [`functions/[[path]].ts`](functions/[[path]].ts) as a
catch-all Pages Function. Deploy with `wrangler pages deploy` (or the Git integration) and
set the same variables/secrets in the Pages project settings. Pick one target — Worker or
Pages — you don't need both.

## Security notes

- **Signed state.** `state` is HMAC-signed and expires (10 min), so a forged or replayed
  callback is rejected before any code is exchanged.
- **CSRF double-submit.** The nonce inside `state` must match an `HttpOnly` cookie set at
  login, defeating login-CSRF.
- **Open-redirect guard.** A `?redirect=` is honoured only if its origin is on the
  allow-list (`APP_URL` + `ALLOWED_REDIRECT_ORIGINS`); everything else falls back to
  `APP_URL`. The gate is the *origin*, so any path under an allowed origin is fine (e.g.
  `http://app.adi/.../callback`).
- **Local `.adi` over http.** Public redirect targets must be `https:`, but the local ADI
  app is served over plain http on the `.adi` split-DNS zone, so http is permitted for
  `.adi` hosts and loopback only — they're reachable solely on the trusted local network,
  and the origin allow-list still applies.
- **Confidential client.** The client secret lives only in the Worker; the code exchange is
  server-to-server. (PKCE isn't required here; it can be layered on per provider later.)
