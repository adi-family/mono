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
- List: `{{cli}} tools list` or `GET /api/tools`. Panel: `/tools`.
- Create (owned): `{{cli}} tools add <name> [--runtime sh|ts] [--description …] [--project <id>]`, or
  `POST /api/tools/create` (`{ "name", "runtime": "sh|ts", "project" }`) — writes a starter
  script you then edit in the store file editor.
- Link (existing file): `{{cli}} tools link <path> [--name …] [--project <id>]`, or
  `POST /api/tools/link` (`{ "path", "name", "runtime", "project" }`).
- Archive / remove: `POST /api/tools/archive` · `/remove` — both regenerate the `.bin` shims.

## Write one whenever you need one
A step you would otherwise repeat by hand — a shell incantation, a query, an API call, a bit of
parsing — is a tool. Creating one is cheap, and it's how the capability outlives the run: the
next agent gets it on its PATH with its own help text instead of rediscovering it. Prefer a new
small tool over pasting the same commands into another prompt.

## Creating it is only half — enable it on the agent
A registered tool is **not** automatically usable by anyone. Nothing is force-fed: an agent gets
a tool only when that tool is ticked on in the agent's own definition (`bin_tools`), which is
what materializes its shim in `tools/.agent-bin/<agent>/` — the directory prepended to that
agent's PATH — and what folds its `llm help` into that agent's system prompt. An agent with no
tools enabled has an empty bin, however many tools the store holds.

So after creating a tool, do the second half:

- Panel: `/agents` → the agent → tick the tool in its tools list, and save.
- CLI: `{{cli}} agents save <agent> --backend <b> --tool <tool-id>` (repeatable, or comma-separated).
- API: `POST /api/agents/save` with `"bin_tools": ["<tool-id>", …]`.

Two things worth knowing: `bin_tools` is a **top-level field** on a save, not one of the backend
`arguments` — a save that omits it un-ticks every tool the agent had. And the enabled set is read
at **launch**, so an agent already running keeps the bin it started with; the next run picks up
the change. (`adi-agent`, the environment's meta-agent, is the exception by design: it is saved
with every active tool enabled.)

## Global, or filed under a project
A tool is either **global** — environment-wide — or **filed under a project** (`--project <id>`,
or `"project": "<id>"` on create/link), which is where it belongs when it only makes sense for
that project's code, data, or API. A project-scoped tool:

- runs with that project's directory as its working directory, and `ADI_TOOL_PROJECT` set;
- gets that project's database rather than the shared one (see `db.md`);
- stays out of the global `tools/.bin` — it reaches an agent only by being enabled on that agent;
- is listed on its own with `{{cli}} tools list --project <id>`.

Default to filing a tool under the project it serves; keep it global only when several projects
would use it.

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
- Creating a tool never gives it to anyone — enable it on each agent that should have it (see
  above and `agents.md`); that's what puts the `.bin` shim on its PATH and its help in the
  agent's prompt.
- Keep tools small and single-purpose — an agent composes them, so one clear job each beats a
  do-everything script.
