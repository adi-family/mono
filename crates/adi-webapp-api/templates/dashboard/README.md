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
widget/               >>> optional: home-screen widgets, one page each <<<
.adi/hive.yaml        the hive services, sharing one host       (do not edit)
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
| `ctx.api.base` | `/api` — the backend's prefix on *this* origin, never a URL |
| `ctx.dashboard` | this dashboard's id |

A module that throws renders its error in its own card and never blocks the others.

### Relative paths only — never a host, never a port

This dashboard is **one origin**: the page and its backend answer on the same hostname, the
backend under `/api`. So a panel asks for `ctx.api.get("/status")` and nothing else. If you need
`fetch` directly, give it a path (`fetch("/api/status")`).

Never write an absolute URL into a panel. Not `http://127.0.0.1:1234`, not `http://<name>.adi`,
not a port from anywhere. The same page is served under this machine's `.adi` name, under
`<label>.<node>.n.adi` when somebody views it from another machine over the mesh, and behind a
real domain later — a hardcoded address pins it to one of those, and `127.0.0.1` in particular
means *the viewer's* machine, which is the one place the backend certainly is not.

### Style with the shell's tokens — never a hex, never a one-off size

The shell carries ADI's design tokens (`design/tokens.css`, spliced in when the dashboard is
generated), so a panel inherits the whole palette and type scale for free. **Use them. Never write
a colour, a family, a radius or a font size of your own.**

| want | use |
| --- | --- |
| page / panel / hover / input surface | `--bg` `--bg-side` `--bg-hover` `--bg-raise` `--bg-active` |
| text, secondary, labels and meta | `--ink` `--ink-2` `--ink-3` |
| hairline between rows, input border | `--line` `--line-strong` |
| status — as a 6px dot or a pill, never a fill | `--ok` `--warn` `--err` |
| the one accent, at most one filled element per screen | `--accent` |
| type sizes | `--fs-label` 12 · `--fs-small` 13 · `--fs-ui-sm` 13.5 · `--fs-ui` 14 · `--fs-section` 16 · `--fs-title` 22 |
| radius | `--r-sm` 4 · `--r` 6 · `--r-lg` 10 · `--r-pill` |
| families | `--sans`, and `--mono` **only** for paths, hashes, commands, ids, config values, model names |

A token that is not in that file does not exist, however plausible it reads. This is not
hypothetical: panels on this machine went on naming `--muted`, `--online`, `--down` and `--panel`
long after those were renamed, and because an undefined custom property simply inherits, every
quiet line rendered at full brightness with nothing in the markup to explain why the screen looked
loud. If you need a colour the token file does not name, say so rather than inventing one.

The rules the tokens serve are `~/adi-family/design/DESIGN.md` — §7 is the checklist that defines
"done", §8 is the Never/Always list. The short version for a panel: group with hairlines and
surface tone rather than by putting a card inside a card, never a card around a table, no shadows
and no gradients, sentence case everywhere, and mono only for strings a machine produced.

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

Route files write plain paths: `path = "/mine"`, reached by the page as `/api/mine`. The mount
prefix is stripped before matching, so it never appears in a route file.

`ctx.params` holds the segments after the route's own path (`/mine/42` → `["42"]`), plus
`ctx.url` (the request URL as it arrived, prefix included — read the query off it, don't match on
it) and `ctx.dashboard`. There are no CORS headers, and none are needed: the page is same-origin.

Routes are loaded at startup; after adding one, `curl http://<this dashboard>/api/_reload` picks
it up without a restart. `/api/_routes` lists what is currently served, `/api/health` is the
liveness probe the shell polls.

## Add a home-screen widget

A widget is a live piece of this app on the ADI home screen. It is its own entry point, beside
`frontend/` and `backend/`: create `widget/<id>.html` and name it in `config.toml`.

```toml
[widget.status]
name = "Status"              # what the home screen calls it; the app's name when absent
url = "/widget/status"       # a path on this app's origin — never a host
size = "medium"              # small (2x2 tiles) | medium (4x2) | large (4x4) | half (the right half)
```

The first time the control panel lists the dashboard after `widget/` appears, it writes
`widget/index.ts` (do not edit) and a third hive service, `widget`, claiming `/widget` on this
host. That server serves `widget/<id>.html` at `/widget/<id>`, `widget/<name>.ts` as
`/widget/<name>.js`, any other file in `widget/` as itself, and the design tokens at
`/widget/tokens.css`.

```html
<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<link rel="stylesheet" href="/widget/tokens.css">
<style>html, body { height: 100%; margin: 0; background: none; }</style>
</head>
<body>
<div id="root"></div>
<script type="module">
  if (new URLSearchParams(location.search).has("light")) document.documentElement.classList.add("light");
  const status = await fetch("/api/status").then((r) => r.json());
  document.getElementById("root").textContent = status.ok ? "Up" : "Down";
</script>
</body>
</html>
```

- It is the same origin as the app: call the backend at `/api/…`, and reuse a panel's code by
  importing the frontend's `/modules/<id>.js` rather than copying it.
- Paint no background and declare no `color-scheme`: the home screen frames the widget on its
  own frosted ground, which has to show through.
- Framed with `?light` while the home screen is light — add the `light` class to `<html>` then,
  and the tokens switch.
- Showing a widget starts the app, like opening it does.

## How it runs

Two hive services, both supervised by the per-user `family.adi.app.dashboards` LaunchAgent, sharing
**one hostname** (`.adi/hive.yaml` declares the same `proxy.host` on both):

- **frontend** — the page itself, owning `/` on that host
- **backend** — the JSON API the page calls, claiming `/api` on the same host
- **widget** — only when `widget/` exists: the home-screen widgets, claiming `/widget`

One origin is the contract, not an implementation detail: it is what lets the page use relative
URLs only, and so work unchanged for a viewer on another machine (over the mesh the dashboard
appears as `<label>.<node>.n.adi`) or behind a real domain later. Open the dashboard by its
hostname — by port, nothing routes `/api` and the page reports the backend as down. The
Dashboards page in the control panel (<http://app.adi/extended/dashboards>) links it.

No port is hardcoded anywhere, and no port ever reaches the browser. adi-hive leases one per
service from the ports manager and injects it as `$PORT`; both are private to the two bun
processes.

```sh
# logs for both servers
tail -f ~/Library/Logs/adi-dashboards.log

# restart both (e.g. after editing an index.ts)
launchctl kickstart -k gui/$(id -u)/family.adi.app.dashboards
```
