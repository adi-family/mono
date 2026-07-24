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

## Notes
- pty backends keep no run history — their live session *is* the run. harness/process backends
  produce answerable turns you can reply to.
- To give an agent tools, list them in its definition; see `tools.md`.
