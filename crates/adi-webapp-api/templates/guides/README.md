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

Everything is under the mono store at `~/.adi/mono` and browsable in the control panel at
`http://app.adi`. Prefer the `adi` CLI and the `/api/*` endpoints over editing store files by
hand; read state first (a `GET`, or `adi … list`) before you change it.
