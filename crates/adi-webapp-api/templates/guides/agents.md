# Agents

An agent is a definition — a backend (runtime), a model, a system prompt, allowed tools, and
optional secrets — stored under `~/.adi/mono/agents/`. `adi-agent` is the default one (this
environment's root agent); everything else is an ordinary definition.

## Backends (runtimes)
- `pty:claude` / `pty:codex` — the CLI in a live terminal session (uses your subscription login).
- `process:claude` / `process:codex` — the same CLI headless, one `--print` turn.
- `harness:claude-sdk` — `claude --print` under ADI's harness (a turn cap + scoped tools).
- `harness:adi` — ADI's own agent loop, multi-provider (needs a provider API key).
- `wasm:*` — a Workforce employee whose loop is set by its own config.

## Do it
- List: `{{cli}} agents list` or `GET /api/agents`. Panel: `/agents`.
- Create / update: `POST /api/agents/save` — the same call the onboarding and Meta page use.
- Run: `POST /api/agents/run`; reply into a conversation with `/api/agents/run/reply`; peek at
  a live run with `/api/agents/run/peek`.
- Sessions land under `~/.adi/mono/sessions/{process,harness}/<agent>/`.

## Tools must be enabled per agent
Registering a tool does not hand it to anybody. Each agent carries its own enabled set —
`bin_tools`, the tool ids ticked on its definition — and only those become shims in the agent's
`.bin` (on its PATH) and only their help is folded into its prompt. Creating a tool and expecting
an agent to have it is the common mistake: create it, then enable it.

- Panel: `/agents` → the agent → tick the tool, save.
- CLI: `{{cli}} agents save <name> --backend <b> --tool <tool-id>` (repeatable / comma-separated).
- API: `POST /api/agents/save` with `"bin_tools": [...]` — a **top-level** field, not one of the
  backend `arguments`. A save that omits it clears the agent's tools.

The set is resolved at launch, so a running agent keeps the bin it started with; the next run
picks up the change. `adi-agent` is saved with every active tool enabled — it operates the whole
environment. See `tools.md`.

## The run environment (`path` / `env`)
A run is started by the daemon, so it inherits launchd's minimal environment — a bare `PATH` like
`/usr/bin:/bin`. ADI rebuilds that `PATH` at launch (the agent's `.bin`, then the standard user and
system tool dirs), but it cannot guess a *project's* pinned toolchain. That is what these two
top-level fields are for; without them an agent ends up prefixing `export PATH=…` onto every shell
command it runs, which only works for as long as it remembers to.

```toml
# ~/.adi/mono/agents/<name>.toml
path = ["$HOME/.nvm/versions/node/v22.14.0/bin"]

[env]
NODE_ENV = "development"
```

- `path` — dirs searched **ahead of the machine's own**, so a project on node 22 beats the system
  node 26. `~` and `$HOME` are expanded at launch. The agent's own `.bin` still comes first.
- `env` — plain variables, injected under their literal names. An entry here wins over an attached
  secret of the same name. `PATH` cannot be set here: it is assembled at launch from `path`.
- Panel: `/agents` → the agent → **Extra PATH dirs** / **Extra environment** (one per line).
- CLI: `{{cli}} agents save <name> --backend <b> --path <dir> --env KEY=VALUE` (both repeatable;
  `--no-path` / `--no-env` clear them).
- API: `POST /api/agents/save` with `"path": [...]` / `"env": {...}`. Unlike `bin_tools`, **omitting
  these keeps what the agent has** — only the full agent form states them — so send `[]` / `{}` to
  clear.

## Notes
- pty backends keep no run history — their live session *is* the run. harness/process backends
  produce answerable turns you can reply to.
