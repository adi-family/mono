# Dashboards

A dashboard is a pair of Bun services — a frontend and a backend — under
`~/.adi/mono/dashboards/<id>/`, authored as loose `.ts` files. The per-user dashboards hive
supervises them, and the front door gives each dashboard its own `<host>.adi` address.

## One origin — relative paths only
A dashboard is **one hostname**. Both services declare the same `proxy.host`: the frontend owns
`/`, the backend claims `/api`. The page must therefore use relative URLs and nothing else.

- A panel calls `ctx.api.get("/status")`; a bare `fetch` takes a path (`fetch("/api/status")`).
- **Never** write a host or a port into a panel, a route, or the shell — not
  `http://127.0.0.1:<port>`, not `http://<name>.adi`. Do not look a port up in the ports
  registry, and do not pass one to the browser.
- Why: the same page is served under `<host>.adi` locally, under `<host>.<node>.n.adi` when it is
  viewed from another machine over the mesh, and behind a real domain later. An absolute URL pins
  it to one of those, and `127.0.0.1` names *the viewer's* machine — the one place the backend is
  not. Relative URLs work for every viewer with no substitution.
- Route files keep writing plain paths (`path = "/mine"`); the backend strips the `/api` mount
  prefix before matching. Reach a running dashboard's API as `http://<host>.adi/api/<route>`.
- Existing dashboards are migrated to this shape the next time they are listed, so a dashboard
  you did not create may have been on the old port-based scheme moments ago. Re-read its
  `.adi/hive.yaml` rather than assuming.

## Where it lives
- `frontend/index.ts`, `frontend/index.html` — the frontend entry points.
- `frontend/modules/*.ts` — one UI panel per file.
- `backend/index.ts` — the backend entry.
- `backend/routes/*.ts` — one HTTP endpoint per file.
- `README.md` — the scaffold's own notes on each extension point.

## Do it
- List: `GET /api/dashboards`. Panel: `/dashboards`.
- Create: `POST /api/dashboards/create` (`{ "name", "description", "template" }`). `template`
  is a template id; omit it (or send `""`) for `plain`, the dependency-free scaffold. This runs
  the template's command and leases both ports right away (`GET /api/ports` shows them
  immediately) — not running processes. Both services come up **on-demand** by default: the
  supervisor starts them on the dashboard's first request and idle-stops them an hour after the
  last one. A leased port is not the same as a running process; check
  `~/.adi/mono/logs/adi-dashboards.log` to see whether the Bun servers are actually up. Don't
  hand-pick ports.
- Edit: change the `.ts` files in place — the dashboard **hot-reloads**. Add a panel by
  dropping a `frontend/modules/<name>.ts`; add an endpoint with a `backend/routes/<name>.ts`.
- Archive / delete: `POST /api/dashboards/archive`, then `/delete` (delete refuses unless the
  dashboard is archived first).

## Templates
A dashboard isn't scaffolded by compiled-in code — it's created by running a shell command a
template names, under `~/.adi/mono/templates/<id>/template.toml`:

```toml
label    = "Vite + React 19 + Hono"
engine   = "bash"
requires = ["bun"]
command  = 'bun create adi-family/dashboard-vite-react "$ADI_DASHBOARD_DIR" && bun "$ADI_DASHBOARD_DIR/scripts/adi-init.ts"'
```

- `id` is the directory name, never a field in the file.
- `engine` names how `command` runs. `"bash"` (`bash -c '<command>'`) is the only one this
  build understands.
- `requires` lists the CLIs `command` needs on `PATH`. Checked before anything runs — a missing
  one refuses with no side effect.
- `command` is the full shell command, run as-is. Nothing here substitutes a placeholder into
  its text; a template reads what it needs from the environment below. There is no `ref` or
  version field — a template pins whatever version it wants inside `command` itself.

`command` runs with:

| Variable | Value |
| --- | --- |
| `ADI_DASHBOARD_DIR` | the directory the command must create |
| `ADI_DASHBOARD_ID` | the new dashboard's id |
| `ADI_DASHBOARD_HOST` | its `<id>.adi` host |
| `ADI_TEMPLATE_DIR` | `~/.adi/mono/templates/<id>/`, for a template's own `files/` |

The command creates the whole dashboard, including its own `.adi/hive.yaml` — the handler only
writes `config.toml` (`name`, `description`, `[created_from]`) once the command succeeds.
`[created_from]` (`template`, `command`) is purely informational; nothing reads it back, and
nothing rewrites a created dashboard afterwards.

Two templates are included: `plain` and `vite-react`. They run `bun create` against
`adi-family/dashboard-plain` and `adi-family/dashboard-vite-react`, respectively. List what's
installed, and whether this machine has what each one needs, with
`GET /api/dashboards/templates`.

Create-time failures:
- An unknown `template` id is a 400 listing the ids that do exist.
- A missing `requires` CLI is a 400 naming it — refused before the command runs.
- A command that exits non-zero is a 500: the half-made directory is removed, and the response
  carries the command's stdout/stderr.

The marketplace's `template` kind installs a template by landing a folder straight into
`templates/`.

## Notes
- Pin a dashboard always-up: add `start: always` next to `restart: always` on **both** services
  in the dashboard's own `.adi/hive.yaml`. Needed whenever something other than this install's
  front door or mesh gateway proxies to the dashboard's ports — those two wake an on-demand
  service on request; nothing else does.
- Read the dashboard's own `README.md` before adding modules — it shows the exact shape a
  panel or route module must export.
- A dashboard predating templates (and only one of those) carries stamped entry points
  (`frontend/index.ts`, `frontend/index.html`, `backend/index.ts`) and a stamped
  `.adi/hive.yaml`; those still get rewritten on migration, the next time the dashboard is
  listed. A template-created dashboard's files carry no stamp and are never touched by it — put
  your work in `frontend/modules/*.ts` and `backend/routes/*.ts` either way, which migration
  never reads.
