// adi-frontend-entry: 1 — this file's generation. The control panel replaces any copy that does
// not spell the current one (`handlers/dashboards.rs`); bump it whenever this file changes. It is
// generated, so a hand-edited one is replaced — panels belong in `modules/`, which is never read
// by migration.
//
// Dashboard frontend — a dependency-free Bun server for agent-authored UI modules.
//
// Everything under `modules/` is discovered at request time, transpiled TS -> browser JS on
// the fly, and mounted by the shell in `index.html`. Adding a panel to this dashboard means
// dropping one `.ts` file into `modules/`; nothing here needs editing, and there is no build
// step and no package.json.
//
// One dashboard is one origin. The hive runner injects $PORT (the port leased for
// `<dashboard>/frontend`), but that number is private to this process: the browser reaches the
// dashboard through the single hostname both hive services share, where this server owns `/` and
// the sibling backend owns `/api`. So this file resolves no address, injects no address, and the
// page it serves is never told one — every request the page makes is a relative path.
//
// That is the whole point, and it is not cosmetic. An earlier version of this file read the
// backend's leased port out of the ports-manager registry and handed it to the browser as
// `127.0.0.1:<port>`. Viewed from another machine — over the mesh, as `<label>.<node>.n.adi` —
// that address is the *viewer's* loopback, not this one, so the page only ever worked when it was
// opened on the machine it ran on. Relative URLs work for every viewer, under every hostname,
// with no substitution.

import { readdir } from "node:fs/promises";
import { basename, join } from "node:path";

const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 8091);
const ROOT = import.meta.dir;
const MODULES = join(ROOT, "modules");

/** This dashboard's id — the directory name the hive service is keyed under. */
const DASHBOARD = basename(join(ROOT, ".."));

/** The prefix the sibling backend claims on this dashboard's host (`proxy.path` in hive.yaml). */
const API_PREFIX = "/api";

/** Module ids available to the shell: every `modules/*.ts`, minus dotfiles, sorted. */
async function moduleIds(): Promise<string[]> {
  try {
    const entries = await readdir(MODULES);
    return entries
      .filter((f) => f.endsWith(".ts") && !f.startsWith("."))
      .map((f) => f.slice(0, -3))
      .sort();
  } catch {
    return [];
  }
}

const transpiler = new Bun.Transpiler({ loader: "ts", target: "browser" });

/** Transpile one `modules/<id>.ts` to browser JS. Returns null if it does not exist. */
async function moduleSource(id: string): Promise<string | null> {
  // `id` reaches us from the URL, so confine it to a single safe path segment before joining.
  if (!/^[A-Za-z0-9._-]+$/.test(id) || id.includes("..")) return null;
  const file = Bun.file(join(MODULES, `${id}.ts`));
  if (!(await file.exists())) return null;
  return transpiler.transformSync(await file.text());
}

const server = Bun.serve({
  port: PORT,
  hostname: "127.0.0.1",
  async fetch(req) {
    const { pathname } = new URL(req.url);

    // The shell, with the little runtime config it genuinely needs: which dashboard this is and
    // which modules to mount. Deliberately no address of any kind — see the header.
    if (pathname === "/" || pathname === "/index.html") {
      const shell = await Bun.file(join(ROOT, "index.html")).text();
      const config = {
        dashboard: DASHBOARD,
        modules: await moduleIds(),
      };
      return new Response(
        shell.replace("__ADI_CONFIG__", JSON.stringify(config)),
        { headers: { "content-type": "text/html; charset=utf-8" } },
      );
    }

    // Live module list, so the shell can refresh without a reload.
    if (pathname === "/modules.json") {
      return Response.json({ modules: await moduleIds() });
    }

    // Agent-authored panels, transpiled on demand.
    if (pathname.startsWith("/modules/")) {
      const id = pathname.slice("/modules/".length).replace(/\.js$/, "");
      const js = await moduleSource(id);
      if (js === null) return new Response("no such module", { status: 404 });
      return new Response(js, {
        headers: {
          "content-type": "text/javascript; charset=utf-8",
          // Modules change as agents write them; never let the browser hold a stale copy.
          "cache-control": "no-store",
        },
      });
    }

    // `/api` belongs to the backend, and the front door routes it there — this server only ever
    // sees it when the page was opened on this process's port instead of on the dashboard's
    // hostname. Say so, rather than 404-ing as if the route did not exist.
    if (pathname === API_PREFIX || pathname.startsWith(`${API_PREFIX}/`)) {
      return Response.json(
        {
          error: "the backend is served under /api on this dashboard's host",
          hint: "open the dashboard by hostname (its .adi name), not by port",
        },
        { status: 404 },
      );
    }

    // Anything else: a static file from the frontend dir.
    const asset = Bun.file(join(ROOT, pathname.replace(/^\/+/, "")));
    if (await asset.exists()) return new Response(asset);
    return new Response("not found", { status: 404 });
  },
});

console.log(
  `dashboard ${DASHBOARD} frontend on http://${server.hostname}:${server.port}`,
);
