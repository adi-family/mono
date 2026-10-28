# {{NAME}}

`{{ID}}`

A dashboard built on Vite + React + TypeScript, Tailwind v4, shadcn/ui, TanStack Query and Hono
on Bun — the full-dependency sibling of the platform's dependency-free dashboard template, for
when an app's UI or API outgrows one file per panel and one file per route.

```
frontend/              a Vite + React app, its own Bun package
  index.html            the HTML shell                                (do not edit)
  vite.config.ts         dev-server wiring: port, allowedHosts, HMR     (do not edit)
  src/
    main.tsx              entry — mounts <App/>                        (do not edit)
    App.tsx                >>> agents add <Route>s here <<<
    index.css              Tailwind v4 + shadcn's CSS variables
    lib/
      api.ts                the typed backend client — hc<AppType>("/api")
      utils.ts              shadcn's `cn()` helper
    components/ui/          >>> shadcn components live here <<<
    pages/                  >>> agents add pages here <<<
backend/                a Hono app on Bun, its own Bun package
  src/
    index.ts                entry — runs `app.ts` on the leased port   (do not edit)
    app.ts                  >>> agents mount routes here (one line) <<<
    routes/                 >>> agents add endpoints here <<<
.adi/hive.yaml          the hive services, sharing one host             (do not edit)
```

Only the four marked "do not edit" files are fixed. Everything a user sees or calls comes from
`src/pages/`, `src/App.tsx`, `src/app.ts` and `src/routes/`.

## How types flow from the backend to the frontend

This is the one real difference from the dependency-free template, where dropping a file into
`backend/routes/` is the whole job. Here, a route becomes part of a single static type —
`AppType` — that the frontend's client is checked against, so **adding an endpoint is two
edits**:

1. Create `backend/src/routes/<name>.ts`, exporting a `Hono()` instance registered at `"/"` (see
   `routes/status.ts` or, for a validated `POST`, `routes/echo.ts`):

   ```ts
   import { Hono } from "hono";

   export const mineRoute = new Hono().get("/", (c) => c.json({ ok: true }));
   ```

2. Mount it in `backend/src/app.ts`, the one file an agent edits to wire a route in:

   ```ts
   const app = new Hono()
     .route("/status", statusRoute)
     .route("/mine", mineRoute); // <- add this
   ```

That's it — `AppType` (`export type AppType = typeof app` in `app.ts`) now includes `/mine`, and
`frontend/src/lib/api.ts`'s `hc<AppType>("/api")` client gets a matching, typed `api.mine.$get()`
for free, with no separate schema and no code generation step.

**Route files must stay framework-agnostic.** `app.ts` is imported for its *type* by the
frontend's own typecheck (see `lib/api.ts`), which has no `Bun` or `process` global — a route
file that calls `Bun.version` or `process.uptime()` will typecheck fine in the backend and then
break the frontend's `tsc` with "Cannot find name". Stick to Hono, zod and Web-platform globals
(`Date`, `crypto`, …) in anything reachable from `app.ts`; put anything Bun-specific in
`backend/src/index.ts`, which is never part of that type.

### Calling it from the frontend

```tsx
import { useQuery } from "@tanstack/react-query";
import { api } from "@/lib/api";

const { data } = useQuery({
  queryKey: ["mine"],
  queryFn: () => api.mine.$get().then((r) => r.json()),
});
```

`api` is `hc<AppType>("/api")` — every property after it is a path segment, and `$get`/`$post`/…
are the Hono RPC client's method calls, inferred from what `app.ts` actually mounts.

### Relative paths only — never a host, never a port

Exactly the same contract as the dependency-free template (`guides/dashboards.md`): this
dashboard is **one origin**, the frontend owns `/` and the backend claims `/api` on the same
host. `hc<AppType>("/api")` is already relative — never change it to an absolute URL, and never
read a port out of anywhere. The same page is served under this machine's `.adi` name, under
`<label>.<node>.n.adi` over the mesh, and behind a real domain later; an absolute address would
pin it to one of those.

## Add a page

Create `frontend/src/pages/<Name>.tsx`, then add a `<Route>` for it in `frontend/src/App.tsx`:

```tsx
<Route path="/mine" element={<Mine />} />
```

## Add a shadcn/ui component

This scaffold ships only `button` and `card` (`frontend/src/components/ui/`). Add another with
the shadcn CLI, from `frontend/`:

```sh
bunx shadcn@latest add dialog
```

It reads `components.json`, writes `src/components/ui/dialog.tsx`, and installs whatever Radix
package that component needs into `frontend/package.json` — nothing here needs editing by hand
first.

## Run it, test it, build it

Every command below runs from `frontend/` or `backend/` respectively — each is its own Bun
package with its own `node_modules`, installed once with `bun install` when the dashboard was
created (re-run it yourself after editing either `package.json`).

```sh
# backend/
bun test        # the example in src/routes/status.test.ts
bun run typecheck

# frontend/
bun run typecheck   # tsc -b --noEmit
bun run build        # tsc -b && vite build — must pass on a fresh scaffold
```

## How it runs

Two hive services, both supervised by the per-user dashboards hive, sharing **one hostname**
(`.adi/hive.yaml` declares the same `proxy.host` on both) — exactly the dependency-free
template's shape, except each service's `working_dir` is its own package directory and its `run`
is that package's `dev` script (`vite` for the frontend, a watching Bun process for the backend)
instead of a bare file path:

- **frontend** — `bun run dev` in `frontend/`, Vite's own dev server, owning `/` on the host
- **backend** — `bun run dev` in `backend/`, Hono on Bun with `--watch`, claiming `/api`

No port is hardcoded anywhere: adi-hive leases one per service and injects it as `$PORT` —
`vite.config.ts` and `backend/src/index.ts` read it from there. Vite's HMR WebSocket needs no
port of its own either; see the comment in `vite.config.ts` for why.
