# Secrets

A secret is one `KEY → value`, stored **encrypted at rest** under `~/.adi/mono/secrets/`, and
injected as an environment variable into an agent's runs. Two scopes: **global** (available
everywhere) or filed under a **project** (a project secret overrides a global of the same name for
that project's runs). A value is either **typed** or **obtained through an OAuth flow** (e.g. Gmail
via Google). The list only ever shows names and descriptions — a value crosses the wire solely on an
explicit reveal/read, never on listing.

## Where it lives
- `~/.adi/mono/secrets/global/<NAME>.toml` — a global secret (encrypted).
- `~/.adi/mono/secrets/projects/<project-id>/<NAME>.toml` — a project-scoped secret.
- `<NAME>` is an env-var identifier (letters, digits, `_`; no leading digit) — it's the exact name
  the value injects as into a run.

## Do it
- List: `{{cli}} secrets list [--project <id>]` or `GET /api/secrets`. Panel: `/extended/secrets`.
- Set (typed): `{{cli}} secrets set <NAME> [value] [--description <d>] [--project <id>]` (value from
  stdin when omitted), or `POST /api/secrets/set` (`{ "name", "value", "description?", "project?" }`).
- Read the value (raw bytes, `op`-style): `{{cli}} secrets read <NAME> [--project <id>]`. This is the
  value-returning primitive an agent uses — no `--reveal` flag. `GMAIL=$({{cli}} secrets read GMAIL_TOKEN)`.
  Over HTTP the same thing is `POST /api/secrets/reveal` (`{ "name", "project?" }`) →
  `{ "name", "project", "value" }` — there is no `GET` that returns a value, and no `/read` route.
- Remove: `{{cli}} secrets rm <NAME>` or `POST /api/secrets/remove`.
- Attach to an agent: tick the secret on the agent's definition (see `agents.md`). Only the ticked
  secrets are decrypted and injected into that agent's runs — an explicit allowlist, not the scope.

## Google / Gmail via OAuth (no Google Cloud setup)
The value can come from an OAuth flow, so the human never handles a raw token. On
`/extended/secrets`:
1. Name the secret (e.g. `GMAIL_TOKEN`), switch the source toggle to **OAuth**, pick **Google**.
2. Tick the access it needs — the Gmail scopes offered are: read (`gmail.readonly`), send
   (`gmail.send`), read+send+manage (`gmail.modify`), full incl. permanent delete
   (`https://mail.google.com/`), and account email (`email`). Request the **least** that does the job
   (a mail client that only reads inboxes wants `gmail.readonly`; add `gmail.send` only to send).
3. Click **Authorize with Google**. The browser goes to `oauth-router.withadi.dev`, the human signs
   in, and returns here with the access token stored — plus a refresh token, so it renews itself.

No Google Cloud Console project or client credentials are needed: the router already holds them.
(Caveat: for sensitive Gmail scopes Google may only let an **allowed test user** of the router's app
through — if authorize is blocked, that account has to be added as a test user.) Any other Google
API works the same way once its scope is offered; the same flow also covers GitHub.

An OAuth secret shows a provider + expiry badge and can be **Refreshed** (server-side, from its
stored refresh token) or **Re-authorized** from its row's menu. Reading one returns its **current
access token**, refreshed server-side when it has expired — so read it fresh at the point of use
rather than caching it in a variable a long-running job keeps reusing.

## Notes
- **Least privilege.** `{{cli}} secrets read` (and the `sys-secrets` tool) returns any secret's value to
  a shell-capable agent — enable that tool deliberately. For a run, prefer the per-agent secret
  allowlist: attach exactly the secrets it needs and they arrive as env vars, nothing else.
- Prefer OAuth over pasting a long-lived token: it scopes access and refreshes itself.
- A project secret shadows a global of the same name for that project's runs — name deliberately.
