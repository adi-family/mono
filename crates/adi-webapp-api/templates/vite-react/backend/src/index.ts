// adi-vite-backend-entry: 1 — this file's generation. The control panel replaces any copy that
// does not spell the current one (`handlers/dashboards.rs`); bump it whenever this file changes.
// It is generated, so a hand-edited one is replaced — endpoints belong in `routes/` plus one
// `.route()` line in `app.ts`, which migration never reads.
//
// Dashboard backend — Hono on Bun. `app.ts` assembles the typed route tree and exports `AppType`;
// this file only runs it, on the port the hive runner leases, after stripping the `/api` mount
// prefix the front door forwards requests with — so route files keep writing plain paths
// (`new Hono().get("/", ...)`, mounted at `/status` by `app.ts`) and reach the browser as
// `/api/status` while still answering at `/status` when hit directly during development.
//
// No CORS headers: one origin (`guides/dashboards.md`) means there is no cross-origin caller to
// allow, and a wildcard allow-origin on a routable hostname would let any page you visit read
// this dashboard's API.

import app from "./app";

const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 8092);
const API_PREFIX = "/api";

function withoutApiPrefix(pathname: string): string {
  if (pathname === API_PREFIX) return "/";
  if (pathname.startsWith(`${API_PREFIX}/`)) return pathname.slice(API_PREFIX.length);
  return pathname;
}

Bun.serve({
  port: PORT,
  hostname: "127.0.0.1",
  fetch(req) {
    const url = new URL(req.url);
    const rewritten = new URL(req.url);
    rewritten.pathname = withoutApiPrefix(url.pathname);
    return app.fetch(new Request(rewritten, req));
  },
});

console.log(`dashboard backend on http://127.0.0.1:${PORT}`);
