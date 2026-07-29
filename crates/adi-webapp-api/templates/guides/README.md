# ADI guides

These are working guides for the moving parts of your ADI stack. **Before you build or
change something in one of these areas, read its guide first** — each one carries the current
store paths, the CLI and API to use, and a worked example, so you act with this environment's
conventions instead of guessing.

Guides live in `~/.adi/mono/guides/` and are plain Markdown. Edit them as your setup evolves;
a later run of the agent picks up the changes.

| Guide | Read it when you… |
| --- | --- |
| `projects.md` | register or restructure a unit of work |
| `dashboards.md` | build or edit a dashboard (frontend + backend) |
| `tasks.md` | track work in the task tree |
| `services.md` | run a long-lived hive service a project supervises |
| `triggers.md` | wire a webhook / background / event code block |
| `tools.md` | give agents a small `sh`/`ts` CLI to run |
| `agents.md` | define or run an agent |
| `secrets.md` | store a secret, connect Gmail/Google via OAuth, or send a create form |
| `db.md` | store data that outlives a run, or that another agent or a dashboard reads |

Everything is under the mono store at `~/.adi/mono` and browsable in the control panel at
`http://app.adi`. Prefer the `{{cli}}` CLI and the `/api/*` endpoints over editing store files by
hand; read state first (a `GET`, or `{{cli}} … list`) before you change it.

## The CLI is `{{cli}}`, and only `{{cli}}`

Every command in these guides starts with `{{cli}}` — that is the ADI CLI, and it is on your
PATH. A machine may also carry an unrelated older binary named plain `adi`, left over from a
previous generation of this stack. It answers a *different* set of commands, so most of what
you know fails against it with `✕ Unknown command: …`, which reads as if the feature were
missing rather than as if you typed the wrong program. Worse are the names that overlap: it has
its own `hive`, and asking that one to bring services down stops things you did not mean.
**Never type `adi` — type `{{cli}}`.**

Two ways to reach the same thing, both fine:

- `{{cli}} <area> <command>` — always available (`{{cli}} tasks list`, `{{cli}} secrets read X`).
- `adi-<area> <command>` — the per-area shims (`adi-tasks list`), present only when that tool is
  enabled on you. Their help is folded into your system prompt, so if you can see it, you have it.
