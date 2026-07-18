// Dashboard backend — a dependency-free Bun server for agent-authored routes.
//
// Every `routes/*.ts` becomes an endpoint. A route file default-exports a handler and may
// export `method` and `path` to override the defaults; `routes/foo.ts` serves `GET /foo`
// unless it says otherwise. Adding an endpoint means dropping one `.ts` file into `routes/`;
// nothing here needs editing.
//
// The hive runner injects $PORT (the port allocated for `<dashboard>/backend`). This service
// has no proxy host of its own, so the browser reaches it directly by port from the frontend's
// origin — which makes every response cross-origin, hence the CORS headers below.

import { readdir } from "node:fs/promises";
import { basename, join } from "node:path";

const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 8092);
const ROOT = import.meta.dir;
const ROUTES = join(ROOT, "routes");

/** This dashboard's id — the directory name the hive service is keyed under. */
const DASHBOARD = basename(join(ROOT, ".."));

/** The browser loads the UI from a different origin (the frontend's host), so allow it. */
const CORS: Record<string, string> = {
  "access-control-allow-origin": "*",
  "access-control-allow-methods": "GET, POST, PUT, PATCH, DELETE, OPTIONS",
  "access-control-allow-headers": "content-type",
};

type Handler = (req: Request, ctx: RouteContext) => Response | Promise<Response>;

interface RouteContext {
  /** Path segments after the route's own prefix, e.g. `/items/42` on `/items` -> `["42"]`. */
  params: string[];
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

    if (req.method === "OPTIONS") return new Response(null, { status: 204, headers: CORS });

    // Liveness — the shell polls this to show backend up/down.
    if (url.pathname === "/health") {
      return Response.json(
        { ok: true, dashboard: DASHBOARD, port: PORT, routes: routes.length },
        { headers: CORS },
      );
    }

    // Pick up newly written route files without a restart.
    if (url.pathname === "/_reload") {
      routes = await loadRoutes();
      return Response.json(
        { reloaded: routes.map((r) => `${r.method} ${r.path}`) },
        { headers: CORS },
      );
    }

    // What this backend currently exposes.
    if (url.pathname === "/_routes") {
      return Response.json(
        { routes: routes.map((r) => ({ method: r.method, path: r.path, source: r.source })) },
        { headers: CORS },
      );
    }

    const hit = match(req.method, url.pathname);
    if (!hit) return new Response("not found", { status: 404, headers: CORS });

    const [route, params] = hit;
    try {
      const res = await route.handler(req, { params, url, dashboard: DASHBOARD });
      // Handlers build their own Response; add CORS without clobbering their headers.
      const headers = new Headers(res.headers);
      for (const [k, v] of Object.entries(CORS)) headers.set(k, v);
      return new Response(res.body, { status: res.status, headers });
    } catch (err) {
      console.error(`route ${route.source} threw:`, err);
      return Response.json({ error: String(err) }, { status: 500, headers: CORS });
    }
  },
});

console.log(
  `dashboard ${DASHBOARD} backend on http://${server.hostname}:${server.port} ` +
    `(${routes.length} route(s))`,
);
