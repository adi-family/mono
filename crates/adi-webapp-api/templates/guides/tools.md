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

## Say what you do — `llm help`
An agent is *told* about its tools: at launch, every tool enabled on it is asked to describe
itself, and the answers are appended to that agent's system prompt. So a tool becomes usable by
documenting itself, not by anyone editing a prompt.

Three forms are tried, in order, and the first that exits `0` with output wins:

1. `<tool> llm help` — help written for a model: what the tool is for, the exact commands and
   flags, what it prints, and one worked example. Write this one.
2. `<tool> help` — what a CLI with subcommands already prints.
3. `<tool> --help` — what a CLI without subcommands prints.

A tool that answers none of them still appears in the prompt by name and description, so the
agent knows it exists — it just has to run it to find out how. Keep the help short: it is capped
(~3 KB per tool) and it is paid for on every turn.

Captures are cached under `tools/.help/<id>` and refreshed when the script changes (or hourly),
so this costs a launch nothing once warm.

## Notes
- To let an agent use a tool, add it to the agent's definition (see `agents.md`); that's what
  puts the `.bin` shim on its PATH — and what puts its help in the agent's prompt.
- Keep tools small and single-purpose — an agent composes them, so one clear job each beats a
  do-everything script.
