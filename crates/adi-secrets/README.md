# adi-secrets

Encrypted, scoped secrets for the ADI platform. Each secret is one AEAD-encrypted
`<name>.toml` under `~/.adi/mono/secrets/`, in one of two scopes:

- **global** — `secrets/global/<NAME>.toml`, available everywhere.
- **per-project** — `secrets/projects/<project-id>/<NAME>.toml`, scoped to an `adi-projects`
  id, and **overriding** a global secret of the same name when a run resolves its environment.

Scope is the platform-wide `project: Option<&str>` convention — `None` is global. A secret's
**name is an environment-variable name** (`[A-Za-z_][A-Za-z0-9_]*`); its value is delivered to
runs under that literal name.

## Encryption

Values are sealed with **XChaCha20-Poly1305**: a 256-bit master key, a fresh random 192-bit
nonce per write, and the secret's own location (`"<scope>/<name>"`) as additional authenticated
data — so a ciphertext copied into a different file, or a tampered tag, fails to decrypt rather
than returning a wrong or foreign value. Metadata (name, description, timestamps) is plaintext;
only the value is ciphertext. Files are written `0600`, their directories `0700`.

Only `reveal(scope, name)` and `resolve(project)` ever produce a plaintext value;
`list`/`get` return metadata alone, and the `Secret` view struct has no value field, so
metadata can't leak a value.

## ⚠️ Master key

The master key is a 32-byte value in a `0600` key-file at `~/.adi/mono/secrets/.master-key`
(base64), generated on first use. It is **not** in the macOS keychain: secrets are injected into
**headless** runs (the `adi-app` LaunchAgent, detached trigger/agent children), which a keychain
prompt would block.

The key sits beside the ciphertext, so this defeats plaintext-in-TOML, accidental commits, and
casual reads — **not** a full-store backup that captures the key too. If you sync or back up
`~/.adi/mono`, either exclude `.master-key`, or relocate the key outside the store with
`ADI_SECRETS_KEY_FILE=/path/to/key`. Losing/rotating the key strands every existing ciphertext,
so a present-but-invalid key file is an error, never silently regenerated.

## OAuth secrets

A secret's value can be a plain string **or** a token obtained through an OAuth flow. For the
latter, `set_oauth(scope, name, OAuthToken, description)` stores:

- the **access token** as the secret's own encrypted value — so it injects into runs and reveals
  exactly like any other secret (a `GITHUB_TOKEN` secret still injects as `$GITHUB_TOKEN`);
- the **refresh token** encrypted separately (distinct AAD), never surfaced to clients — only
  `reveal_refresh` reads it, server-side, to mint a new access token;
- plaintext **metadata**: provider, `obtained_at`, `expires_at`, `scope`, and whether a refresh
  token is held.

The `Secret` view carries an `OAuthInfo` (provider / lifetime / `has_refresh`) but never a token.
A plain `set` on an OAuth secret drops its provenance (it becomes a text secret again).

The browser obtains the tokens through the [`oauth-router`](../../apps/oauth-router) worker (user
runs the provider flow; the token returns in the redirect fragment), and the app posts them to
`set_oauth`. Renewal is server-side: `adi-app` decrypts the refresh token, exchanges it at the
router's `/refresh/<provider>` (which holds the client secret), and re-stores the fresh token —
the refresh token never reaches the browser.

## Injection into runs

A run inherits `resolve(project)` — every global secret plus the run's project's (project
overrides global) — as env vars under their literal names, a namespace distinct from the
`ADI_<KEY>` trigger-settings convention. Reserved platform vars (`PATH`, `ADI_TRIGGER*`) always
win, so a secret can't shadow them. Wired into `adi-triggers` (fire + supervised background) and
`adi-agents` (the detached process/harness backends and the tmux session).

Resolving an empty store touches no key, so a system that never uses secrets never materializes
one just by firing triggers or launching agents.

## Surfaces

- **Library**: `Secrets::{open, set, get, reveal, list, list_all, remove, resolve}`.
- **CLI**: `adi-mono secrets {set,get,list,rm}` with `--global` / `--project <id>` and
  `--reveal` on `get`. `set` reads the value from an argument or stdin.
- **HTTP API** (`adi-app`): `GET /api/secrets`, `POST /api/secrets/{set,remove,reveal}`.
- **Web UI**: a Secrets page and a per-project panel (values masked; reveal is deliberate).
