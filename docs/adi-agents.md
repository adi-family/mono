# adi-agents

`adi-agents` stores reusable agent definitions. It does not run agents yet.

The command center for agent and task state is the `adi-mono` CLI:

```bash
adi-mono agents list
adi-mono agents save planner --backend cli:codex --command-scope tasks,projects
adi-mono tasks add "Investigate auth flow" --project demo --tag planner
adi-mono tasks complete DEMO-1
```

The web app can create and view definitions/tasks, but deeper task mutations and agent
administration should stay in `adi-mono` so there is one scriptable control surface.

## Current Scope

Implemented:

- `adi-agents`: definition store under `~/.adi/mono/agents`, one TOML file per agent.
- `adi-tasks`: task tree under `~/.adi/mono/tasks/tasks.json`.
- `adi-mono agents ...`: list, show, save, and delete definitions.
- `adi-mono tasks ...`: list, add, show, edit, complete, archive, reopen, and delete tasks.
- Web app pages for agent definitions and task visibility.

Not implemented yet:

- Running an agent process.
- Session history and live event streaming.
- Auto-starting an agent from a tagged task.
- Backend adapters for `cli:codex`, `cli:claude`, or API providers.
- Permission enforcement for command scopes.

## Definition Model

An agent definition is stored as:

```rust
pub struct AgentManifest {
    pub backend: String,          // cli:codex, cli:claude, api:openai, ...
    pub system_prompt: String,
    pub tools: String,            // historical field; now used as CLI command scope
    pub model: Option<String>,
    pub permission_mode: Option<String>,
    pub temperature: Option<f64>,
    pub max_turns: Option<u32>,
    pub tags: Vec<String>,
    pub starred: bool,
    pub extra: BTreeMap<String, String>,
    pub created_at: u64,
    pub updated_at: u64,
}
```

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

Backends should be CLI-first:

- `cli:codex` and `cli:claude` are subprocess adapters controlled by `adi-mono`.
- API backends can be added later, but they should still be driven from the same command center.
- The backend contract should emit a common event stream: started, stdout/stderr, tool/command
  request, task update, completed, failed.

The command scope in the manifest is the allow-list the runner uses before exposing or executing
commands for an agent. The initial implementation can be conservative and only allow known
high-level groups (`tasks`, `projects`, `agents`) before adding finer-grained controls.

## Recommended Next Steps

1. Add an `adi-mono agents run <name> --task <id>` command.
2. Define a small backend trait in a new orchestration crate or in `adi-agents` once execution
   starts.
3. Persist sessions under `~/.adi/mono/sessions`.
4. Add an auto-start loop that only claims `ready` tagged tasks.
5. Enforce command scope in the runner before any command is invoked.
