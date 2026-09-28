// adi-widget-entry: 1 — this file's generation. The control panel replaces any copy that does
// not spell the current one (`handlers/dashboards.rs`); bump it whenever this file changes. It is
// generated, so a hand-edited one is replaced — widgets belong beside it in `widget/`, which
// migration never reads.
//
// Dashboard widgets — a dependency-free Bun server for the pages this app offers the ADI home
// screen. The third entry point beside `frontend/` and `backend/`, on the same host, claiming
// `/widget` (`proxy.path` in the dashboard's hive.yaml, written because `widget/` exists).
//
// A widget is one page: `widget/<id>.html`, served at `/widget/<id>`. The home screen learns it
// exists from the app's `config.toml`, never from this server, so it can draw an app's widget
// slot without starting the app:
//
//   [widget.chat]
//   name = "Chat"
//   url = "/widget/chat"
//   size = "half"            # small | medium | large | half
//
// Alongside the pages:
//   /widget/tokens.css       the ADI design tokens, `.light` included — link it, never copy it
//   /widget/<name>.js        `widget/<name>.ts`, transpiled on request
//   /widget/<file>           any other file in `widget/`
// and, because this is one origin, the app's own `/api/*` and `/modules/<id>.js` are a relative
// URL away — a widget reuses the frontend's modules rather than keeping a copy.
//
// The home screen frames a widget with `?light` while it is light: add the `light` class to
// `<html>` then. A widget paints no background and declares no `color-scheme`, so the home
// screen's frosted ground shows through it.

import { basename, join, normalize } from "node:path";

const PORT = Number(process.env.PORT ?? process.env.PORT_HTTP ?? 8093);
const ROOT = import.meta.dir;

/** This dashboard's id — the directory name the hive service is keyed under. */
const DASHBOARD = basename(join(ROOT, ".."));

/** The prefix this server is mounted under on the dashboard's host. */
const WIDGET_PREFIX = "/widget";

/** `design/tokens.css`, spliced in by the control panel when it writes this file. */
const TOKENS: string = "/* @adi-tokens */";

const transpiler = new Bun.Transpiler({ loader: "ts", target: "browser" });

/**
 * The file a request path names inside `widget/`, or null when it would leave it. Tolerates both
 * shapes, like the backend does: `/widget/chat` through the front door, `/chat` against the
 * process's own port while developing.
 */
function fileFor(pathname: string): string | null {
  let rel = pathname;
  if (rel === WIDGET_PREFIX) rel = "/";
  else if (rel.startsWith(`${WIDGET_PREFIX}/`)) rel = rel.slice(WIDGET_PREFIX.length);
  rel = normalize(decodeURIComponent(rel)).replace(/^\/+/, "");
  if (rel.startsWith("..") || rel.split("/").some((part) => part.startsWith("."))) return null;
  return rel;
}

const server = Bun.serve({
  port: PORT,
  hostname: "127.0.0.1",
  async fetch(req) {
    const rel = fileFor(new URL(req.url).pathname);
    if (rel === null) return new Response("not found", { status: 404 });

    if (rel === "tokens.css") {
      return new Response(TOKENS, { headers: { "content-type": "text/css; charset=utf-8" } });
    }

    // A widget by its id: `/widget/chat` is `widget/chat.html`.
    if (rel && !rel.includes(".")) {
      const page = Bun.file(join(ROOT, `${rel}.html`));
      if (await page.exists()) {
        return new Response(page, {
          headers: { "content-type": "text/html; charset=utf-8", "cache-control": "no-store" },
        });
      }
    }

    // A script, authored as TypeScript.
    if (rel.endsWith(".js")) {
      const source = Bun.file(join(ROOT, `${rel.slice(0, -3)}.ts`));
      if (await source.exists()) {
        return new Response(transpiler.transformSync(await source.text()), {
          headers: {
            "content-type": "text/javascript; charset=utf-8",
            // Widgets change as agents write them; never let the browser hold a stale copy.
            "cache-control": "no-store",
          },
        });
      }
    }

    // This entry point is the one file here that is not the widget's own.
    if (rel && rel !== "index.ts") {
      const asset = Bun.file(join(ROOT, rel));
      if (await asset.exists()) return new Response(asset);
    }
    return new Response("not found", { status: 404 });
  },
});

console.log(`dashboard ${DASHBOARD} widgets on http://${server.hostname}:${server.port}`);
