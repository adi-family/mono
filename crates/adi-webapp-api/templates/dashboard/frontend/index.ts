// Dashboard frontend — a dependency-free Bun server for agent-authored UI modules.
//
// Everything under `modules/` is discovered at request time, transpiled TS -> browser JS on
// the fly, and mounted by the shell in `index.html`. Adding a panel to this dashboard means
// dropping one `.ts` file into `modules/`; nothing here needs editing, and there is no build
// step and no package.json.
//
// The hive runner injects $PORT (the port allocated for `<dashboard>/frontend`). The backend
// runs as a sibling hive service on its own port with no proxy host of its own, so the browser
// talks to it directly by port — see `backendPort()`.

import { readdir } from "node:fs/promises";
import { basename, join } from "node:path";

const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 8091);
const ROOT = import.meta.dir;
const MODULES = join(ROOT, "modules");

/** This dashboard's id — the directory name the hive service is keyed under. */
const DASHBOARD = basename(join(ROOT, ".."));

/**
 * The sibling backend's port.
 *
 * `$BACKEND_PORT` wins when set. Otherwise we read the ports manager's registry, the same
 * source of truth adi-hive allocated from, so neither port is ever hardcoded. Returns null if
 * the backend has not been allocated yet; the UI degrades to "backend offline" rather than
 * failing to boot.
 */
async function backendPort(): Promise<number | null> {
  const fromEnv = Number(process.env.BACKEND_PORT);
  if (Number.isFinite(fromEnv) && fromEnv > 0) return fromEnv;

  const adiDir = process.env.ADI_DIR?.trim() || ".adi";
  const registry = join(
    process.env.HOME ?? "",
    adiDir,
    "mono",
    "ports",
    "registry.json",
  );
  try {
    const { leases } = await Bun.file(registry).json();
    const lease = leases?.find(
      (l: { service: string; key: string }) =>
        l.service === `${DASHBOARD}/backend` && l.key === "http",
    );
    return lease?.port ?? null;
  } catch {
    return null;
  }
}

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

    // The shell, with its runtime config injected so the browser knows where the backend is.
    if (pathname === "/" || pathname === "/index.html") {
      const shell = await Bun.file(join(ROOT, "index.html")).text();
      const config = {
        dashboard: DASHBOARD,
        backendPort: await backendPort(),
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

    // Anything else: a static file from the frontend dir.
    const asset = Bun.file(join(ROOT, pathname.replace(/^\/+/, "")));
    if (await asset.exists()) return new Response(asset);
    return new Response("not found", { status: 404 });
  },
});

console.log(
  `dashboard ${DASHBOARD} frontend on http://${server.hostname}:${server.port}`,
);
