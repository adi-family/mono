# employees/ — where our workforce employees live

One TypeScript file per employee. Each file is compiled to a WASM component
(TS → esbuild → jco) and dispatched by the `adi-workforce` engine bundled
into `adi-agents` (backend `wasm:loop-script`).

`examples/workforce/` holds throwaway engine probes; real employees go here.

## Write

Copy `hello.ts`, rename the file and the `sdk.register({ name })`. The SDK is
`@adi-family/workforce-sdk` (local `sdk/workforce`); bundled capabilities the
engine provides:

| plugin id                             | surface                                          |
| ------------------------------------- | ------------------------------------------------ |
| `adi.workforce.runner.claude`         | runner `ClaudeCodeApi({ model })` — Anthropic API, auth like Claude Code |
| `adi.workforce.capability.shell`      | tool `Shell` |
| `adi.workforce.capability.tasks`      | tools `TaskCreate/TaskGet/TaskList/TaskUpdate/TaskResolve` |
| `adi.workforce.capability.orchestration` | tool `MessageEmployee`, trigger `EmployeeMessage` |
| `adi.workforce.variable.env`          | tool `Env` |
| `adi.workforce.filesystem.sandbox`    | filesystem `Sandbox({ id })` |

Note: a loop's `systemMessage` doubles as its first user turn — put the
request text in it (or seed richer state via `prependHistory`).

## Build

```bash
cd employees
npm install        # once
npm run build      # every *.ts → build/<name>.wasm
```

## Register + run

Register the compiled component as an agent (once):

```bash
adi-mono agents save hello \
  --backend wasm:loop-script \
  --extra wasm=$PWD/build/hello.wasm
```

or in the web UI (app.adi → Agents): backend **wasm · Workforce employee**,
**Source path** = the absolute `.ts` path (Component path fills in on first Build).

## Edit in the web UI

On app.adi → Agents, every wasm agent row has a **{ } Code** action: it opens the
`.ts` source (the agent's `src` extra) in an editor panel with **Save**, **⚙ Build**
(saves first when dirty, then compiles server-side and shows the build output), and
**Reload**. The first successful Build fills in an empty Component path automatically.

Dispatch a message into it:

```bash
adi-mono agents run hello -m "say hi"
```

`--handler` picks a specific trigger subscription; the default is the
employee's first one. Engine logs land under `~/.adi/mono/workforce/`.

After editing a `.ts` file, rebuild — the agent definition keeps pointing at
the same `build/<name>.wasm` path, so no re-registration is needed.
