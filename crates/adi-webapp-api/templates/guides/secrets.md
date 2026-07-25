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
- List: `adi secrets list [--project <id>]` or `GET /api/secrets`. Panel: `/extended/secrets`.
- Set (typed): `adi secrets set <NAME> [value] [--description <d>] [--project <id>]` (value from
  stdin when omitted), or `POST /api/secrets/set` (`{ "name", "value", "description?", "project?" }`).
- Read the value (raw bytes, `op`-style): `adi secrets read <NAME> [--project <id>]`. This is the
  value-returning primitive an agent uses — no `--reveal` flag. `GMAIL=$(adi secrets read GMAIL_TOKEN)`.
- Remove: `adi secrets rm <NAME>` or `POST /api/secrets/remove`.
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
stored refresh token) or **Re-authorized** from its row's menu.

## Send the user a ready-to-submit form
In a **conversation** (a harness/answerable agent), you can drop a form straight into your reply and
the chat renders it live — prefilled — so the human only reviews and clicks. Emit a fenced
`adi-form` block whose body is this JSON:

- `title` (required), `description?`, `submit_label?`.
- `action` — `{ "type": "set_secret" }` for a typed secret, or
  `{ "type": "oauth_secret", "provider": "google", "scopes": ["…"] }` to start the OAuth flow.
- `fields[]` — each `{ "name", "label", "type", "value?", "placeholder?", "hint?", "required?",
  "mono?", "options?" }`. `type` is one of `text`, `textarea`, `number`, `select`, `checkbox`,
  `secret` (masked). For secret actions the well-known field names are `name`, `value`,
  `description`, `project`; prefill them via `value`.

Gmail (OAuth) — the human just clicks Authorize:

```adi-form
{
  "title": "Create the Gmail secret",
  "description": "Read access to your Gmail. You'll sign in with Google, then land back here with the token stored.",
  "action": { "type": "oauth_secret", "provider": "google", "scopes": ["https://www.googleapis.com/auth/gmail.readonly", "email"] },
  "fields": [
    { "name": "name", "label": "Secret name", "type": "text", "value": "GMAIL_TOKEN", "mono": true, "hint": "the env-var name it injects as" },
    { "name": "project", "label": "Project", "type": "text", "value": "", "placeholder": "(global — a project id scopes it)", "mono": true },
    { "name": "description", "label": "What it's for", "type": "text", "value": "Gmail read access for the mail client" }
  ]
}
```

A typed key the human pastes:

```adi-form
{
  "title": "Save the API key",
  "action": { "type": "set_secret" },
  "fields": [
    { "name": "name", "label": "Secret name", "type": "text", "value": "OPENAI_API_KEY", "mono": true },
    { "name": "value", "label": "Value", "type": "secret", "placeholder": "paste the key" }
  ]
}
```

Submit calls the same endpoints as the Secrets page (`/api/secrets/set`, or the OAuth flow), so the
result appears on `/extended/secrets`. A malformed block just renders as a code block — it's never
silently dropped.

## Notes
- **Least privilege.** `adi secrets read` (and the `sys-secrets` tool) returns any secret's value to
  a shell-capable agent — enable that tool deliberately. For a run, prefer the per-agent secret
  allowlist: attach exactly the secrets it needs and they arrive as env vars, nothing else.
- Prefer OAuth over pasting a long-lived token: it scopes access and refreshes itself.
- A project secret shadows a global of the same name for that project's runs — name deliberately.
