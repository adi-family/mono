# adi-agents

`adi-agents` stores reusable agent definitions and launches interactive tmux or detached headless
process backends. Deeper orchestration (session history, events, auto-start) does not exist yet.

The command center for agent and task state is the `adi-mono` CLI:

```bash
adi-mono agents list
adi-mono agents save planner --backend tmux:codex --argument sandbox=workspace-write
adi-mono agents run planner        # detached tmux session adi-agent-planner
adi-mono agents save reviewer --backend process:codex --argument sandbox=read-only
adi-mono agents run reviewer --message "Review the current branch" # background codex exec
adi-mono tasks add "Investigate auth flow" --project demo --tag planner
adi-mono tasks complete DEMO-1
```

The web app can create and view definitions/tasks, but deeper task mutations and agent
administration should stay in `adi-mono` so there is one scriptable control surface.

## Current Scope

Implemented:

- `adi-agents`: definition store under `~/.adi/mono/agents`, one TOML file per agent.
- `adi-agents`: a `tmux` launcher — `Agents::run` starts an interactive engine CLI detached in an
  `adi-agent-<name>` tmux session; attach with `tmux attach -t adi-agent-<name>`.
- `adi-agents`: a `process` launcher — `Agents::run_with_message` starts `claude --print` or
  `codex exec` detached in its own process group, recording a PID and combined log under
  `~/.adi/mono/sessions/process/`.
- `adi-agents`: a `harness` launcher — `harness:claude-sdk` runs the `claude` CLI headless
  through the same detached machinery (recording a PID/log under `~/.adi/mono/sessions/harness/`),
  adding a turn cap (`--max-turns`) and folding the agent's adi-mono command scope into its system
  prompt. `harness:adi` is typed and stored but not yet runnable.
- `adi-tasks`: task tree under `~/.adi/mono/tasks/tasks.json`.
- `adi-mono agents ...`: list, show, save, run, stop, and delete definitions.
- `adi-mono tasks ...`: list, add, show, edit, complete, archive, reopen, and delete tasks.
- Web app pages for agent definitions and task visibility; ▶ Run launches a supported backend
  and shows live sessions/processes. Tmux runs additionally expose ● View (`tmux capture-pane`
  plus `tmux send-keys`); process runs remain non-interactive. ■ Stop uses the executor lifecycle.

Not implemented yet:

- The `harness:adi` loop engine (the backend is typed and stored, but not runnable).
- Structured session history and live event streaming (process output is currently a flat log).
- Auto-starting an agent from a tagged task.
- Permission enforcement for command scopes.

## Definition Model

An agent definition is stored as:

```rust
pub struct AgentManifest<Args> {
    pub backend: String,          // executor:what — tmux:claude, process:codex, harness:adi, ...
    pub arguments: Args,          // the backend's strict argument type
    pub tags: Vec<String>,
    pub starred: bool,
    pub project: Option<String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

For example, a cloud backend defines `CloudAgentArguments` and uses
`AgentManifest<CloudAgentArguments>`. Misspelled fields and wrong value types are then compiler or
deserialization errors, not string-key lookups. `AgentManifest<Args>` implements `Default` whenever
`Args` does.

`arguments` owns the settings interpreted by the selected backend. Vendor CLI schemas contain only
options their command builders use; harness and WASM schemas may also carry `tools`, `max_turns`,
provider settings, and source paths. `AgentManifest` itself contains only fields ADI uses to file,
dispatch, and timestamp the definition. Built-in executors reject unknown fields.

The registry uses `StoredAgentManifest` only at its heterogeneous storage/listing boundary because
one directory can contain unrelated backend argument types. `Agents::save` accepts and returns the
same typed manifest, while encoding a storage copy; `Agents::get_typed::<Args>` restores that type
when reading it later.
Legacy top-level backend fields and the old string-only `extra` map migrate into `arguments` when
read.

For harness and WASM backends, `tools` is the CLI command scope: for example `tasks`, `projects`,
`agents`, or a comma-separated subset. It is not accepted by vendor CLI backends, whose native
tool controls use fields such as `allowed_tools` and `disallowed_tools`.

## Task Dispatch Direction

Tasks are the dispatch queue shape, not the runner:

- `tag` can match an agent name.
- an open task with no open direct child is `ready`;
- an open task with open direct children is `blocked`;
- `done` and `archived` tasks should not auto-start.

When orchestration exists, the runner should poll or subscribe to `adi-tasks`, select `ready`
tasks whose tag maps to an agent definition, and then launch the configured backend.

## Backend Direction

A backend id is an `executor:what` pair: the executor is the run mechanism, the suffix is the
thing it runs. The executor decides *how* the loop executes; it never names a model provider.

- `tmux:claude` / `tmux:codex` — a vendor agent CLI driven inside a tmux session; the CLI owns
  its own agentic loop, `adi-mono` attaches, observes, and reaps.
- `process:claude` / `process:codex` — the same vendor CLI run headless as a plain subprocess
  (print/exec mode), controlled by `adi-mono`.
- `harness:claude-sdk` — the `claude` CLI run headless by ADI's harness: a turn-capped
  (`--max-turns`), adi-scoped `--print` run, spawned detached like the `process` executor. (A
  fully embedded Claude Agent SDK loop is future work; today it drives the CLI.)
- `harness:adi` — ADI's own agentic loop; *which* model API it calls is the definition's
  `provider` argument (anthropic, openai, gemini, monshoot, ollama), not part of the backend id.
  Typed and stored today, but its loop engine does not exist yet, so it is not runnable.
- Every executor should emit a common event stream: started, stdout/stderr, tool/command
  request, task update, completed, failed.

The command scope in the manifest is the allow-list the runner uses before exposing or executing
commands for an agent. The initial implementation can be conservative and only allow known
high-level groups (`tasks`, `projects`, `agents`) before adding finer-grained controls.

## Recommended Next Steps

1. Grow `adi-mono agents run <name>` a `--task <id>` handoff (seed the session with the task).
2. Build the `harness:adi` loop engine (provider clients + turn loop) behind its typed arguments.
3. Grow `~/.adi/mono/sessions` from process PID/log files into structured session records.
4. Add an auto-start loop that only claims `ready` tagged tasks.
5. Enforce command scope in the runner before any command is invoked.
