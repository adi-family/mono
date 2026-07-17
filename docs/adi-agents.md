# adi-agents

`adi-agents` stores reusable agent definitions and launches interactive tmux or detached headless
process backends. Deeper orchestration (session history, events, auto-start) does not exist yet.

The command center for agent and task state is the `adi-mono` CLI:

```bash
adi-mono agents list
adi-mono agents save planner --backend tmux:codex --command-scope tasks,projects
adi-mono agents run planner        # detached tmux session adi-agent-planner
adi-mono agents save reviewer --backend process:codex --extra sandbox=read-only
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
- `adi-tasks`: task tree under `~/.adi/mono/tasks/tasks.json`.
- `adi-mono agents ...`: list, show, save, run, stop, and delete definitions.
- `adi-mono tasks ...`: list, add, show, edit, complete, archive, reopen, and delete tasks.
- Web app pages for agent definitions and task visibility; ▶ Run launches a supported backend
  and shows live sessions/processes. Tmux runs additionally expose ● View (`tmux capture-pane`
  plus `tmux send-keys`); process runs remain non-interactive. ■ Stop uses the executor lifecycle.

Not implemented yet:

- Executor adapters for the `harness` backends.
- Structured session history and live event streaming (process output is currently a flat log).
- Auto-starting an agent from a tagged task.
- Permission enforcement for command scopes.

## Definition Model

An agent definition is stored as:

```rust
pub struct AgentManifest {
    pub backend: String,          // executor:what — tmux:claude, process:codex, harness:adi, ...
    pub arguments: BTreeMap<String, serde_json::Value>, // structured backend-owned settings
    pub system_prompt: String,
    pub tools: String,            // historical field; now used as CLI command scope
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub temperature: Option<f64>,
    pub max_turns: Option<u32>,
    pub tags: Vec<String>,
    pub starred: bool,
    pub project: Option<String>,
    pub extra: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

`AgentManifest` uses its `Default` implementation whenever a field is omitted while
deserializing. This keeps older and partial manifests readable as the common model grows.
`arguments` is the structured backend-owned extension point; unlike the legacy string-only
`extra` map, it can hold booleans, numbers, lists, and nested backend manifests.

The `tools` field name is retained for manifest compatibility, but its meaning is CLI command
scope: for example `tasks`, `projects`, `agents`, or a comma-separated subset. Future execution
code should treat this as the set of `adi-mono` command groups an agent may use.

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
- `harness:claude-sdk` — an agentic loop embedded via the Claude Agent SDK.
- `harness:adi` — ADI's own agentic loop; *which* model API it calls is the definition's
  `provider` extra (anthropic, openai, gemini, monshoot, ollama), not part of the backend id.
- Every executor should emit a common event stream: started, stdout/stderr, tool/command
  request, task update, completed, failed.

The command scope in the manifest is the allow-list the runner uses before exposing or executing
commands for an agent. The initial implementation can be conservative and only allow known
high-level groups (`tasks`, `projects`, `agents`) before adding finer-grained controls.

## Recommended Next Steps

1. Grow `adi-mono agents run <name>` a `--task <id>` handoff (seed the session with the task).
2. Define a small backend trait over the executor modules, then add the `harness` adapters.
3. Grow `~/.adi/mono/sessions` from process PID/log files into structured session records.
4. Add an auto-start loop that only claims `ready` tagged tasks.
5. Enforce command scope in the runner before any command is invoked.
