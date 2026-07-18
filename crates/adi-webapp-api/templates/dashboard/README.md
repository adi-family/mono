# {{NAME}}

`{{ID}}`

A dashboard an agent can extend by writing files. No dependencies, no build step, no
`package.json` — TypeScript executed directly by bun.

```
frontend/index.ts     entry — serves the shell, transpiles modules   (do not edit)
frontend/index.html   the shell that mounts modules                  (do not edit)
frontend/modules/     >>> agents add UI panels here <<<
backend/index.ts      entry — discovers and serves routes            (do not edit)
backend/routes/       >>> agents add endpoints here <<<
.adi/hive.yaml        the two hive services
```

Only the two `index.ts` files are fixed. Everything a user sees comes from `modules/` and
`routes/`, and both directories are read at request time — no restart to add a panel.

## Add a UI panel

Create `frontend/modules/<name>.ts`. Default-export a function; the shell calls it with a
context and mounts whatever it renders. The file is discovered, transpiled to browser JS, and
loaded on the next page load.

```ts
export default async function myPanel(ctx) {
  const el = ctx.panel("My panel");        // titled card to render into
  const data = await ctx.api.get("/mine"); // pre-bound to the backend
  el.textContent = JSON.stringify(data);
}
```

`ctx` gives you:

| field | what it is |
| --- | --- |
| `ctx.panel(title)` | appends a card to the grid, returns the element to fill |
| `ctx.api.get(path)` | GET a backend route, parsed as JSON |
| `ctx.api.post(path, body)` | POST JSON to a backend route |
| `ctx.api.base` | the backend's origin, or `null` when it is down |
| `ctx.dashboard` | this dashboard's id |

A module that throws renders its error in its own card and never blocks the others.

## Add an endpoint

Create `backend/routes/<name>.ts`. Default-export a handler returning a `Response`. The file
name becomes the path, so `mine.ts` serves `GET /mine`; export `method` / `path` to override.

```ts
export const method = "POST";      // optional, defaults to GET
export const path = "/mine";       // optional, defaults to /<filename>

export default async function mine(req, ctx) {
  return Response.json({ ok: true, params: ctx.params });
}
```

`ctx.params` holds the segments after the route's own path (`/mine/42` → `["42"]`), plus
`ctx.url` and `ctx.dashboard`. CORS is applied for you.

Routes are loaded at startup; after adding one, `curl http://127.0.0.1:$BACKEND/_reload` picks
it up without a restart. `/_routes` lists what is currently served, `/health` is the liveness
probe the shell polls.

## How it runs

Two hive services, both supervised by the per-user `family.adi.dashboards` LaunchAgent:

- **frontend** — the page itself, on `http://127.0.0.1:<frontend port>`
- **backend** — the JSON API the page calls, on its own port

Neither has a hostname: a dashboard depends only on its own supervisor, not on the root front
door or DNS, so adding one never needs a privileged restart. The Dashboards page in the control
panel (<http://app.adi/dashboards>) lists both ports and links the frontend.

Neither port is hardcoded. adi-hive reserves one per service from the ports manager and injects
it as `$PORT`; the frontend looks the backend's port up in the same registry and hands it to
the browser. Set `$BACKEND_PORT` to override.

```sh
# logs for both servers
tail -f ~/Library/Logs/adi-dashboards.log

# restart both (e.g. after editing an index.ts)
launchctl kickstart -k gui/$(id -u)/family.adi.dashboards
```
