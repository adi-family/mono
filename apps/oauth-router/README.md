# adi-oauth-router

A generic **OAuth router**, deployed as a Cloudflare **Pages** project on
**`oauth-router.withadi.dev`**. It fronts many OAuth providers (Google, GitHub, …) behind two
routes, and its whole job is to take the token a provider hands back and **route it to the
app** — by redirecting the browser back to the app with the token in the URL fragment.

```
  browser ──/login/google──▶  router ──302──▶  accounts.google.com
                                                      │  (user approves)
  app  ◀──302 #access_token=…──  router  ◀──/callback/google?code=…──┘
                                   └─ exchanges code for token server-to-server
```

The router holds **no session and no database**. What the callback needs to know — which
provider, where to send the token, a CSRF nonce — is carried across the round-trip in a
signed, expiring `state`, verified on the way back.

Everything runs in one catch-all Pages Function, [`functions/[[path]].ts`](functions/[[path]].ts),
which is a three-line adapter over [`src/router.ts`](src/router.ts). The static root
[`public/`](public/) holds nothing but a `_routes.json` that hands every path to that Function.

## Routes

| Route | What it does |
| --- | --- |
| `GET /` · `GET /health` | JSON: the service name and which providers are configured. |
| `GET /login/<provider>?redirect=<app-url>&scope=<optional>` | Redirects to the provider's consent screen. `redirect` (optional) is where the token is delivered; it must be allow-listed. `scope` (optional) overrides the provider's default scopes. |
| `GET /callback/<provider>?code=…&state=…` | The URL you register with the provider. Exchanges the code and redirects to the app with the token in the fragment. |
| `POST /refresh/<provider>` `{ refresh_token }` | Mints a fresh access token from a stored refresh token, server-to-server (JSON in, JSON out). Lets a saved token renew without the user re-authorizing. The refresh token is the credential, so no extra auth is imposed; the client secret stays server-side. |

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
| `ROUTER_URL` | var | This deployment's own canonical origin. Not optional on Pages — see below. |
| `APP_URL` | var | Default token-delivery target; its origin is always allow-listed. |
| `ALLOWED_REDIRECT_ORIGINS` | var | Extra comma-separated origins a `?redirect=` may target. |
| `INCLUDE_REFRESH_TOKEN` | var | `"true"` to also forward the provider's `refresh_token`. |
| `<PROVIDER>_CLIENT_ID` / `_CLIENT_SECRET` / `_SCOPES` | secret / var | Per-provider credentials and scope override. |

Vars live in [`wrangler.toml`](wrangler.toml) under `[vars]`; secrets are set with
`wrangler pages secret put <NAME> --project-name adi-oauth-router` and never committed. Once
that config file exists, Pages reads `[vars]` from it and **ignores plain-text variables set in
the dashboard** — so the file is the source of truth. Secrets are stored separately and are
unaffected.

### Why `ROUTER_URL` exists

The redirect URI handed to a provider has to be the one registered with it, byte for byte, at
both the authorize hop and the code exchange. A Pages project answers on
`adi-oauth-router.pages.dev` and on a preview host per branch as well as on the custom domain,
and — unlike a Worker's `workers_dev = false` — there is no way to switch those off. So the
router pins its own origin from `ROUTER_URL` rather than reading it off the request's `Host`;
otherwise a login entered through a `pages.dev` host would send Google an unregistered URI and
get `redirect_uri_mismatch`. A `GET /login/…` that arrives on a non-canonical host is bounced
to `ROUTER_URL` first, so the nonce cookie is set on the same host that will read it back.

Leave `ROUTER_URL` out and the request origin is used instead, which is what
`wrangler pages dev` wants — see [`.dev.vars.example`](.dev.vars.example).

## Develop

```bash
cd apps/oauth-router
bun install                 # or: npm install

cp .dev.vars.example .dev.vars   # fill in local secrets (git-ignored)
bun run dev                 # wrangler pages dev — local server on :8788

bun run typecheck           # tsc --noEmit
bun run test                # vitest
```

## Standing it up on a Cloudflare account

Four things have to be true for a login to work, and only the first is in git:

1. the Pages project `adi-oauth-router` exists, with this code deployed to it
2. `STATE_SECRET` and each provider's client id/secret are set as Pages secrets
3. `oauth-router.withadi.dev` is attached to the project as a **custom domain**
4. each provider has `https://oauth-router.withadi.dev/callback/<provider>` registered

Points 2 and 3 are account-scoped and live nowhere in this repo, which is why a Cloudflare
account migration takes the router down even though nothing here changed.
[`scripts/setup-cf.sh`](scripts/setup-cf.sh) does 1–3 and prints what is left of 4. It is
idempotent, so it doubles as the "has anything drifted?" check:

```bash
npx wrangler login
./scripts/setup-cf.sh                            # or: --secrets-from .dev.vars
```

A login is all it needs. **Attaching a custom domain to a Pages project has no `wrangler`
command** — it exists only in the dashboard and the REST API
(`POST /accounts/{id}/pages/projects/{project}/domains`) — but the OAuth token `wrangler login`
stores is accepted as a bearer token there, so the script borrows it rather than asking for a
second credential. `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ACCOUNT_ID` override that if you'd
rather be explicit; the account id is otherwise read from the login.

Three things about the custom domain are worth knowing before you watch it and worry:

- **Attaching the domain does not create the DNS record.** The dashboard wizard offers to; the
  API does not, and the domain then sits at `pending` with `"CNAME record not set"` forever.
  The record is `CNAME oauth-router → adi-oauth-router.pages.dev`, **proxied**. The script
  creates it when the credentials allow — which a `wrangler login` does not, since it carries
  `zone:read` and no DNS scope, so that case prints the one record to add by hand.
- **A 522 in the middle is the expected state, not a fault.** Once the record exists but the
  Pages domain is still `pending`, the edge treats `pages.dev` as an ordinary proxied origin
  and times out reaching it. It starts routing to the project when verification completes.
- Until then `https://adi-oauth-router.pages.dev/health` already answers — that is the useful
  smoke test immediately after a deploy, and the one the script reports.

If verification stalls, `PATCH /accounts/{id}/pages/projects/{project}/domains/{domain}` with
an empty body re-triggers it.

An existing secret is **left alone** on a re-run, which matters because `--secrets-from
.dev.vars` is the convenient way to invoke this and `.dev.vars` is exactly where a placeholder
like `STATE_SECRET="dev-only-change-me"` lives. Pass `--force-secrets` to rotate deliberately.
`STATE_SECRET` is generated when nothing supplies one — nothing else needs to know it, it only
has to be secret and stable.

Routine redeploys afterwards need none of that:

```bash
bun run deploy              # wrangler pages deploy
```

`/health` is the check that matters — it lists the providers that are actually enabled, so a
provider missing from that list is a secret that never got set.

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
- **Confidential client.** The client secret lives only in the Function; the code exchange is
  server-to-server. (PKCE isn't required here; it can be layered on per provider later.)
