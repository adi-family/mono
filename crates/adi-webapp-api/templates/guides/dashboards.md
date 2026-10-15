# Dashboards

A dashboard is a pair of bun services — a frontend and a backend — under
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
- Create: `POST /api/dashboards/create` (`{ "name", "description" }`). This scaffolds the
  files and leases both ports right away (`GET /api/ports` shows them immediately) — but **not**
  running processes. Both services come up **on-demand** by default: the supervisor starts them
  on the dashboard's first request and idle-stops them an hour after the last one. A leased port
  is not the same as a running process; check `~/.adi/mono/logs/adi-dashboards.log` to see
  whether the bun servers are actually up. **Don't** hand-pick ports.
- Edit: change the `.ts` files in place — the dashboard **hot-reloads**. Add a panel by
  dropping a `frontend/modules/<name>.ts`; add an endpoint with a `backend/routes/<name>.ts`.
- Archive / delete: `POST /api/dashboards/archive`, then `/delete` (delete refuses unless the
  dashboard is archived first).

## Notes
- Pin a dashboard always-up: add `start: always` next to `restart: always` on **both** services
  in the dashboard's own `.adi/hive.yaml`. Needed whenever something other than this install's
  front door or mesh gateway proxies to the dashboard's ports — those two wake an on-demand
  service on request; nothing else does.
- Read the dashboard's own `README.md` before adding modules — it shows the exact shape a
  panel or route module must export.
- The four entry points (`frontend/index.ts`, `frontend/index.html`, `backend/index.ts`, and
  `.adi/hive.yaml`) are generated and get rewritten on migration. Put your work in
  `frontend/modules/*.ts` and `backend/routes/*.ts`, which are never touched.
