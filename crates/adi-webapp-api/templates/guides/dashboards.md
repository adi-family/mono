# Dashboards

A dashboard is a pair of bun services — a frontend and a backend — under
`~/.adi/mono/dashboards/<id>/`, authored as loose `.ts` files. The per-user dashboards hive
supervises them, and the front door gives each dashboard its own `<host>.adi` address.

## Where it lives
- `frontend/index.ts`, `frontend/index.html` — the frontend entry points.
- `frontend/modules/*.ts` — one UI panel per file.
- `backend/index.ts` — the backend entry.
- `backend/routes/*.ts` — one HTTP endpoint per file.
- `README.md` — the scaffold's own notes on each extension point.

## Do it
- List: `GET /api/dashboards`. Panel: `/dashboards`.
- Create: `POST /api/dashboards/create` (`{ "name", "description" }`). This scaffolds the
  files; the supervisor then leases ports and starts both bun servers within a few seconds, so
  the new host appears on its own. **Don't** hand-pick ports.
- Edit: change the `.ts` files in place — the dashboard **hot-reloads**. Add a panel by
  dropping a `frontend/modules/<name>.ts`; add an endpoint with a `backend/routes/<name>.ts`.
- Archive / delete: `POST /api/dashboards/archive`, then `/delete` (delete refuses unless the
  dashboard is archived first).

## Notes
- Read the dashboard's own `README.md` before adding modules — it shows the exact shape a
  panel or route module must export.
