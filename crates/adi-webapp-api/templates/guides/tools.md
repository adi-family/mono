# Tools

A tool is a small `sh` or `ts` script that agents run as a CLI, under `~/.adi/mono/tools/`. A
tool is either **owned** (its code lives in the store) or **linked** (a manifest pointing at an
existing file). Each active tool is exposed as a `tools/.bin/<name>` shim, so an agent with that
`.bin` directory on its PATH always sees the live set.

## Where it lives
- `~/.adi/mono/tools/<name>.{sh,ts}` — an owned tool's script.
- `~/.adi/mono/tools/.bin/<name>` — the generated shim (don't edit by hand; it's regenerated on
  every create/edit/archive/remove).

## Do it
- List: `adi tools list` or `GET /api/tools`. Panel: `/tools`.
- Create (owned): `POST /api/tools/create` (`{ "name", "runtime": "sh|ts" }`) — writes a
  starter script you then edit in the store file editor.
- Link (existing file): `POST /api/tools/link` (`{ "path", "name", "runtime" }`).
- Archive / remove: `POST /api/tools/archive` · `/remove` — both regenerate the `.bin` shims.

## Notes
- To let an agent use a tool, add it to the agent's definition (see `agents.md`); that's what
  puts the `.bin` shim on its PATH.
- Keep tools small and single-purpose — an agent composes them, so one clear job each beats a
  do-everything script.
