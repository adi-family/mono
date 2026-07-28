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
- List: `adi agents list` or `GET /api/agents`. Panel: `/agents`.
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
- CLI: `adi agents save <name> --backend <b> --tool <tool-id>` (repeatable / comma-separated).
- API: `POST /api/agents/save` with `"bin_tools": [...]` — a **top-level** field, not one of the
  backend `arguments`. A save that omits it clears the agent's tools.

The set is resolved at launch, so a running agent keeps the bin it started with; the next run
picks up the change. `adi-agent` is saved with every active tool enabled — it operates the whole
environment. See `tools.md`.

## Notes
- pty backends keep no run history — their live session *is* the run. harness/process backends
  produce answerable turns you can reply to.
