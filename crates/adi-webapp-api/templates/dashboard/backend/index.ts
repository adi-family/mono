// adi-backend-entry: 1 — this file's generation. The control panel replaces any copy that does
// not spell the current one (`handlers/dashboards.rs`); bump it whenever this file changes. It is
// generated, so a hand-edited one is replaced — endpoints belong in `routes/`, which is never
// read by migration.
//
// Dashboard backend — a dependency-free Bun server for agent-authored routes.
//
// Every `routes/*.ts` becomes an endpoint. A route file default-exports a handler and may
// export `method` and `path` to override the defaults; `routes/foo.ts` serves `GET /foo`
// unless it says otherwise. Adding an endpoint means dropping one `.ts` file into `routes/`;
// nothing here needs editing.
//
// The hive runner injects $PORT (the port allocated for `<dashboard>/backend`). This service does
// not have a host of its own: it shares the frontend's, claiming the `/api` prefix on it
// (`proxy.path` in the dashboard's hive.yaml), so the page calls it same-origin with relative
// URLs and never learns an address. That is also why there are no CORS headers here — there is no
// cross-origin caller to allow, and a wildcard allow-origin on a routable hostname would let any
// page you happen to visit read this dashboard's API.
//
// The front door forwards the path as it arrived, so requests land here as `/api/<route>`. The
// prefix is stripped below, which keeps route files writing plain paths (`/status`) and keeps the
// backend working unchanged if it is ever mounted at the root instead.

import { readdir } from "node:fs/promises";
import { basename, join } from "node:path";

const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 8092);
const ROOT = import.meta.dir;
const ROUTES = join(ROOT, "routes");

/** This dashboard's id — the directory name the hive service is keyed under. */
const DASHBOARD = basename(join(ROOT, ".."));

/** The prefix this backend is mounted under on the dashboard's host. */
const API_PREFIX = "/api";

/**
 * The path to route on: the request path minus the mount prefix. Tolerates both shapes, so the
 * same file serves `/api/status` through the front door and `/status` when a route is exercised
 * against the process directly (`curl 127.0.0.1:$PORT/status` while developing).
 */
function routePath(pathname: string): string {
  if (pathname === API_PREFIX) return "/";
  if (pathname.startsWith(`${API_PREFIX}/`)) return pathname.slice(API_PREFIX.length);
  return pathname;
}

type Handler = (req: Request, ctx: RouteContext) => Response | Promise<Response>;

interface RouteContext {
  /** Path segments after the route's own prefix, e.g. `/items/42` on `/items` -> `["42"]`. */
  params: string[];
  /** The request URL as it arrived — so `url.pathname` still carries the `/api` mount prefix.
   *  Read the query off it (`url.searchParams`); match on `params`, never on `url.pathname`. */
  url: URL;
  dashboard: string;
}

interface Route {
  method: string;
  path: string;
  handler: Handler;
  source: string;
}

/**
 * Load every `routes/*.ts`. A file that fails to import is reported and skipped, so one broken
 * route never takes the whole backend down with it.
 */
async function loadRoutes(): Promise<Route[]> {
  let files: string[];
  try {
    files = await readdir(ROUTES);
  } catch {
    return [];
  }

  const routes: Route[] = [];
  for (const file of files.sort()) {
    if (!file.endsWith(".ts") || file.startsWith(".")) continue;
    const name = file.slice(0, -3);
    try {
      // Cache-bust so a re-import after an edit picks the new file up.
      const mod = await import(`${join(ROUTES, file)}?v=${Bun.nanoseconds()}`);
      const handler = mod.default ?? mod.handler;
      if (typeof handler !== "function") {
        console.warn(`route ${file}: no default export function; skipped`);
        continue;
      }
      routes.push({
        method: (mod.method ?? "GET").toUpperCase(),
        path: mod.path ?? `/${name}`,
        handler,
        source: file,
      });
    } catch (err) {
      console.warn(`route ${file}: failed to load — ${err}`);
    }
  }
  return routes;
}

let routes = await loadRoutes();

/** Longest-prefix match, so `/items/42` reaches the `/items` route with params `["42"]`. */
function match(method: string, pathname: string): [Route, string[]] | null {
  const candidates = routes
    .filter((r) => r.method === method)
    .filter((r) => pathname === r.path || pathname.startsWith(`${r.path}/`))
    .sort((a, b) => b.path.length - a.path.length);

  const route = candidates[0];
  if (!route) return null;
  const rest = pathname.slice(route.path.length).replace(/^\/+/, "");
  return [route, rest ? rest.split("/") : []];
}

const server = Bun.serve({
  port: PORT,
  hostname: "127.0.0.1",
  async fetch(req) {
    const url = new URL(req.url);
    const pathname = routePath(url.pathname);

    // Liveness — the shell polls this to show backend up/down.
    if (pathname === "/health") {
      return Response.json({
        ok: true,
        dashboard: DASHBOARD,
        port: PORT,
        routes: routes.length,
      });
    }

    // Pick up newly written route files without a restart.
    if (pathname === "/_reload") {
      routes = await loadRoutes();
      return Response.json({ reloaded: routes.map((r) => `${r.method} ${r.path}`) });
    }

    // What this backend currently exposes.
    if (pathname === "/_routes") {
      return Response.json({
        routes: routes.map((r) => ({ method: r.method, path: r.path, source: r.source })),
      });
    }

    const hit = match(req.method, pathname);
    if (!hit) return new Response("not found", { status: 404 });

    const [route, params] = hit;
    try {
      return await route.handler(req, { params, url, dashboard: DASHBOARD });
    } catch (err) {
      console.error(`route ${route.source} threw:`, err);
      return Response.json({ error: String(err) }, { status: 500 });
    }
  },
});

console.log(
  `dashboard ${DASHBOARD} backend on http://${server.hostname}:${server.port} ` +
    `(${routes.length} route(s))`,
);
