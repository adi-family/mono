# adi-workforce — WASM-employee agents

The agent engine ported from the old adi-family repo's `workforce` stack
(`~/projects/adi-family/cli/plugins/adi/workforce`). An agent ("employee")
is a TypeScript file compiled to a WebAssembly *component* and driven by a
Rust wasmtime host. Unlike the old repo, there are **no loadable plugins**:
every capability is compiled into the binary (`adi_workforce::bundled`).

## The pieces

- `crates/adi-workforce` — the engine.
  - `wasm_config_loader.rs` — the wasmtime host (`WasmEmployee`): loads a
    component, reads its `sdk.register(...)` identity, runs `main()` to
    collect trigger subscriptions, dispatches events into handlers, and
    implements the WIT `host` interface (`loop-init` / `loop-llm` /
    `loop-tool` / `loop-finish`, `call-tool`, `log`, `subscribe`, ...).
  - `dispatch.rs` — one-shot run path: install the wasm under the
    `workforce` module dir, instantiate, deliver one message to a handler.
  - `bundled/` — statically registered capabilities, keyed by the same
    plugin-id strings the old dlopen'd plugins used:
    | Plugin id | Provides |
    |---|---|
    | `adi.workforce.runner.claude` | `ClaudeCodeApi` runner: Anthropic Messages API with Claude Code OAuth (keychain) or `ANTHROPIC_API_KEY`, prompt-cache breakpoints, request dumps |
    | `adi.workforce.capability.shell` | `Shell` tool (regex allow/deny, output spill) |
    | `adi.workforce.capability.tasks` | `TaskCreate/TaskGet/TaskList/TaskUpdate/TaskResolve` over **adi-tasks** |
    | `adi.workforce.capability.orchestration` | `MessageEmployee` tool + `EmployeeMessage` trigger (yaque disk queues) |
    | `adi.workforce.variable.env` | `Env` function |
    | `adi.workforce.filesystem.sandbox` | `Init`/`Cleanup` functions + `Sandbox` filesystem |
- `sdk/workforce` — the TypeScript SDK (`@adi-family/workforce-sdk`) and
  `build.mjs` (esbuild bundle → `jco componentize` against
  `workforce.wit`, world `loop-script`). Wizer snapshots module top-level
  at build time, so only `sdk.register(...)` may run there; live code
  belongs in `export const main`.
- `examples/workforce` — `probe.ts`, an LLM-free engine probe. Build with
  `npm run build` (or `node ../../sdk/workforce/build.mjs probe.ts -o build`).

## Loop protocol

TS drives the agentic loop; the host is an LLM/tool execution service.
`sdk.loop({runner, tools, systemMessage, middlewares}).run()` calls
`loop-init` (host instantiates tools, resolves the filesystem, builds the
LLM backend, composes the system prompt), then alternates `loop-llm` (one
model turn, Anthropic Messages wire shape) and `loop-tool` (one tool
execution), then `loop-finish`. Middleware/lifetime hooks run entirely in
TS. An optional `schema` makes the host advertise a synthetic decision
tool that ends the loop with structured output.

## Running an agent

```sh
# 1. Compile the TS agent to a wasm component
cd examples/workforce && npm run build

# 2. Register it as an agent (backend wasm:loop-script + extra.wasm path)
adi-mono agents save probe --backend wasm:loop-script \
  --extra wasm=$PWD/build/probe.wasm

# 3. One-shot dispatch a message into its handler
adi-mono agents run probe -m "hello"
```

Employees are installed under `~/.adi/mono/workforce/<name>/`
(`config.wasm`, `sdk_log.jsonl`, `usage.jsonl`, inbox queues).

## Not ported yet

- The daemon: persistent trigger watchers (cron, inbox watch loops) over
  the subscription list — today `agents run` is a synchronous one-shot
  dispatch. The `EmployeeMessage` queue plumbing is already in place.
- `TaskComment`/`TaskHistory` (no adi-tasks counterpart), the shell tool's
  LLM safety checker (PromptRunner), `SwitchToBranch`, and the
  integration plugins (telegram/github/gitlab/linear).
- Workspaces/init-hook integration: a loop's default workdir is the
  employee dir, not a project workspace (same gap as tmux agents).
