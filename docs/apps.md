# How an ADI app is built

An app — the code calls it a *dashboard* — is one directory under `~/.adi/mono/dashboards/<id>/`,
run by the per-user hive supervisor and reached at `<label>.adi` (and `<label>.<node>.n.adi` from a
paired machine). This is the recommended shape for every app, whether a person, an agent or the
marketplace writes it. The worked example is the Agent board, the home screen's chat
(`crates/adi-dashboards/seeds/agent-board/`).

```
<id>/
  config.toml          what the app is: name, icon, the widgets it offers
  frontend/            entry point 1 — the app's page, at /
    index.ts, index.html   generated (do not edit)
    modules/*.ts           the panels
    favicon.svg            its picture on the home screen
  backend/             entry point 2 — its API, at /api
    index.ts               generated (do not edit)
    routes/*.ts            the endpoints
  widget/              entry point 3, optional — its home-screen widgets, at /widget
    index.ts               generated (do not edit)
    <id>.html              one page per widget
  .adi/hive.yaml       generated: one hive service per entry point
```

The scaffold's README (`crates/adi-webapp-api/templates/dashboard/README.md`, copied into every
new app) is the hands-on guide to each directory. This page is the rules, and why.

## 1. One origin, three entry points

Every entry point is its own hive service, and all of them declare the **same** `proxy.host`: the
frontend owns `/`, the backend claims `/api`, the widget service claims `/widget`. So every page the
app serves — its own and its widgets — calls the backend at `/api/…` and loads the frontend's
modules at `/modules/<id>.js` with relative URLs, and never learns an address. That is what lets
one app work at `nosh.adi`, at `nosh.<node>.n.adi` and behind a real domain, for every viewer
(`docs/fleet.md` §4).

- Never write a host or a port into anything the browser loads.
- A widget reuses the frontend's modules by importing them; it does not keep a copy. The Agent
  board's widget and its page mount the same `frontend/modules/chat.ts`.
- The `index.ts` files are generated and stamped. The control panel replaces any copy that does not
  carry the current stamp, so an edit to one is lost. Everything an app does lives beside them.

## 2. Widgets are declared in `config.toml`

A widget is a live piece of an app on the ADI home screen. The app says which widgets it offers in
its `config.toml`, one table each:

```toml
name = "Agent board"
icon = "message-square"

[widget.chat]
name = "Chat"               # what the home screen calls it; the app's name when absent
url = "/widget/chat"        # a path on the app's origin — never an address
size = "half"               # small (2x2 tiles) | medium (4x2) | large (4x4) | half (right half)
```

- **Declared, not discovered.** The home screen reads `config.toml` as a file — through the panel's
  `/api/fs/read`, on a paired machine through its panel — so it knows an app's widgets without
  starting the app, and works against panels that predate the feature.
- **`url` is a path.** An app answers under a different hostname for every viewer, so the home
  screen puts the path on whichever origin it reaches the app by. A full URL, or `//host/…`, is
  ignored.
- **The page:** `widget/<id>.html`, served at `/widget/<id>`. Link `/widget/tokens.css` (the
  design tokens, spliced in by the panel — never copy the palette), paint no background and declare
  no `color-scheme` (the home screen's frosted ground shows through), and add the `light` class to
  `<html>` when the URL carries `?light`.
- The widget service exists only while `widget/` does: creating the directory is what makes the
  panel write `widget/index.ts` and the service, on its next listing of the app.
- Showing a widget is a request to the app, so it starts a stopped app, as opening it does.
- `config.toml` is the app's own, but the panel rewrites it (archive, import, marketplace) through
  `adi_dashboards::write_manifest`, which keeps `[widget.*]`. A new field there has to be added to
  `Manifest` or the next rewrite drops it.

## 3. `hive.yaml` uses relative paths

The generated `.adi/hive.yaml` names no absolute path:

```yaml
  widget:
    restart: always
    proxy:
      host: agent-board.adi
      path: /widget
    runner:
      type: script
      script:
        run: bun run widget/index.ts
        working_dir: .
```

**`working_dir: .` is the app's own directory, not `.adi/`.** adi-hive resolves an imported
`<root>/.adi/hive.yaml`'s relative paths against `<root>` — it strips the trailing `.adi`
(`import_base_dir` in `crates/adi-hive/src/config.rs`). So `..`, which reads as "up from the file",
is the dashboards root, and nothing starts.

Relative, so a hive file is still true after the directory is moved, restored or copied to another
machine. The panel treats an absolute `working_dir` as out of date and rewrites it to `.`, keeping
the host (`adi_dashboards::is_current`).

Nor does the file declare a port: adi-hive leases one per service from the ports manager, keyed
`<id>/<service>`, and injects it as `$PORT`.

## Where it lives in the code

| what | where |
| --- | --- |
| `config.toml`, `[widget.*]` | `crates/adi-dashboards/src/manifest.rs` |
| the hive file, the widget service, `is_current` | `crates/adi-dashboards/src/hive.rs` |
| the generated entry points and the scaffold README | `crates/adi-webapp-api/templates/dashboard/` |
| migration: stamps, the widget entry, the hive rewrite | `crates/adi-webapp-api/src/handlers/dashboards.rs` |
| the home screen reading and framing widgets | `crates/adi-webapp/src/new_ui/widgets.rs`, `apps.rs` |
