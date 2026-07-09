# adi-agents — design spec

**Status:** draft / spec-first (no code yet — this document is the thing we will implement).
**Home:** `crates/adi-agents` in this workspace (to be created).
**Depends on:** [`adi-mcp`](../crates/adi-mcp) (tool surface), `adi-config`, `adi-projects`, `adi-fs`, and optionally `adi-hive` (supervision).

---

## 1. Goal

`adi-agents` is a general **agent-orchestration layer**: it *creates, spawns, supervises, and
reaps* AI coding/testing agents, feeding each one a **prompt** from a library and a **scoped
tool surface** served by `adi-mcp`. An agent's engine — **codex**, **claude**, a raw provider
API, etc. — is a pluggable **backend**.

This is the same modular idea we used for `adi-mcp` (one runtime, capabilities selected at
launch), pushed one level up: instead of "which tools does this MCP expose," the knob is
"which backend + which prompt + which tools make up this agent."

The design is a **conversion of the logic** already proven in the bug-bounty operator console
at `~/bugbounty/application` — *not* a copy of it. That app works but is Claude-hardcoded,
drives agents by scraping a `tmux`/`ttyd` TUI (racy), exposes tools by injecting `PATH` shims,
and stores state as loose JSON. We keep its *mechanisms* (a prompt library keyed by role, a
task-queue auto-spawner, an idle reaper, per-agent scoping) and replace its *implementation*
with a backend contract, structured (headless) agent I/O, `adi-mcp` for tools, and the
`adi-config`/`adi-fs`/`adi-projects` primitives this workspace already owns.

### Non-goals
- **Prompt content.** The reference app's ~130 WSTG prompts are domain-specific bug-bounty
  briefs. We convert the *prompt-library mechanism*, not the prompts. `adi-agents` ships a
  prompt-library format and a tiny example set; real prompt collections live per project.
- **A new provider SDK.** API backends use existing SDKs / HTTP; CLI backends shell out to the
  vendor binary. `adi-agents` owns orchestration, not model plumbing.
- **Replacing `adi-mcp`.** `adi-mcp` is the tool surface; `adi-agents` is a *consumer* of it.

---

## 2. How the reference system works today (the logic we're converting)

A compact map of `~/bugbounty/application`, so the conversion below is legible. (Source of
truth: its `src/lib/target-agents.ts`, ~1500 lines, is the whole engine.)

| Concern | Reference app (today) | Why it needs converting |
|---|---|---|
| **Engine** | `claude` CLI hardcoded (`~/.local/bin/claude`); model flag configurable (`opus\|sonnet\|fable\|haiku`) | No backend abstraction — adding codex means rewriting flag assembly + activity heuristics |
| **Spawn** | Next.js server → detached `tmux` session running interactive zsh → `send-keys` types the launch line and the prompt; `ttyd` for a browser terminal | TUI-scraping: needs a shell-readiness gate, double-Enter, sentinel bounce; inherently racy; no exit code, no structured events |
| **Prompt** | `prompts/<name>.md` library; the agent's `promptTemplate` is compiled into `--append-system-prompt-file`; a `milestone → agent → prompt` map picks it; runtime task line typed into the TUI | Good idea, keep it — but decouple from Claude-only flags and TUI typing |
| **Tools** | `bin/` CLIs exposed by writing `.bin/<name>` shims onto the agent's `PATH` + injecting each tool's `help-llm` text into the system prompt; MCP intentionally *removed* in favor of this | We already built the better version: `adi-mcp` over stdio, tools scoped via `--features` |
| **Scope** | env-only: `TARGET_SLUG`/`TARGET_DIR` injected; each CLI resolves its data dir from env; agent lives under `data/<slug>/agents/<name>/` | No isolation boundary beyond cwd+env; `adi-fs` jail + `adi-projects` give a real one |
| **Data** | 1 SQLite table (`targets`) + flat JSON/markdown per target; two "session" meanings (tmux run vs. `*.jsonl` transcript) | Re-read from disk on every request; no typed registry |
| **Lifecycle** | boot loops in `instrumentation.ts`: **auto-spawner** (dispatch `todo` tasks to their mapped agent, capacity 3 across targets) + **reaper** (kill idle runs via log-mtime + `capture-pane` state detection: working/waiting/idle/stuck); slots (max 10); starred agents; roster seeding (~130 canonical agents) | Keep the loops; drive them off structured status instead of scraping panes |
| **Control plane** | `tmux send-keys` in / `capture-pane` + log mtime out | Replace with a backend that emits structured events + an exit code |

**Verdict:** the *orchestration logic* is sound and worth porting; the *coupling* (Claude-only,
TUI-scraped, PATH-shimmed, JSON-on-disk) is what we drop.

---

## 3. Concepts / domain model

```
Project (adi-projects)            a target / codebase an agent works on
└─ Workspace (adi-fs Jail)        the project dir the agent is confined to
   └─ AgentDef                    a named, reusable agent definition (spec)
      ├─ backend: BackendRef      which engine runs it (cli:claude, api:anthropic, …)
      ├─ prompt: PromptRef        which prompt from the library seeds it
      ├─ tools: ToolScope         adi-mcp features/tools this agent may use
      ├─ model / params           model alias, temperature, permission mode, env
      └─ role / tags              e.g. "solver", "planner", milestone tag
   └─ Session (a "run")           one live execution of an AgentDef
      ├─ backend-owned process    PTY/headless child OR an in-proc API loop
      ├─ events: stream           structured status + output events
      ├─ transcript               persisted message log
      └─ status                   starting | working | waiting | idle | done | failed | killed
Roster                            the set of AgentDefs seeded into a project
Orchestrator                      spawns/supervises/reaps Sessions; runs the auto-spawn + reap loops
PromptLibrary                     keyed collection of prompt files (role/category → prompt)
Task queue                        units of work the auto-spawner dispatches to agents
```

Note the convergence with what we already have: a reference **"target" is an adi `Project`**;
its **workspace** is an `adi-fs` jail (`adi-mcp`'s `files` feature already gates file access to
*registered* projects); and the **task queue** the auto-spawner drains maps directly onto
`adi-mcp`'s **`tasks` feature**. `adi-agents` largely *wires existing primitives together*.

---

## 4. Architecture & crate layout

Runs as a library plus a thin binary (mirrors `adi-mesh`/`adi-mcp`). Later it can be an
`adi-hive`-supervised service and surface in `adi-app`.

```
crates/adi-agents/
├── Cargo.toml
└── src/
    ├── lib.rs           # re-exports; the orchestrator entry
    ├── main.rs          # `adi-agents` CLI (create/list/run/stop/ps/logs/roster …)
    ├── agent.rs         # AgentDef: the reusable definition (serde, stored via adi-config)
    ├── prompt.rs        # PromptLibrary: keyed prompt collection + resolution
    ├── backend/
    │   ├── mod.rs       # the AgentBackend trait + BackendRef parsing + registry
    │   ├── cli.rs       # CliBackend: spawn a vendor CLI (codex/claude), headless > PTY
    │   └── api.rs       # ApiBackend: an in-proc agent loop over a provider API
    ├── session.rs       # Session: a live run, its event stream, transcript, status
    ├── orchestrator.rs  # spawn / supervise / reap; slots + capacity; auto-spawn loop
    ├── roster.rs        # seed a project's canonical AgentDefs
    ├── workspace.rs     # scope: project + adi-fs jail + injected context/env
    └── store.rs         # persistence over adi-config (AgentDefs, sessions, transcripts)
```

**Dependencies (proposed):** `adi-config` (state), `adi-projects` (targets), `adi-fs`
(workspace jail), `adi-mcp` (the tool surface — see §7), `tokio` (async runtime + process
supervision), `rmcp` (MCP *client*, for API backends to call adi-mcp tools), `serde`/
`serde_json`, `clap`, `tracing`; and for CLI backends a PTY crate (e.g. `portable-pty`) **only
if** a backend requires a TTY (see §6).

---

## 5. Agent definition (`AgentDef`)

A stored, reusable spec — the analogue of the reference app's `AgentSettings` + roster entry,
minus the Claude-specific flag soup.

```rust
// Illustrative, not final.
struct AgentDef {
    name: String,             // unique within a project, e.g. "athz-solver"
    backend: BackendRef,      // "cli:claude" | "cli:codex" | "api:anthropic" | "api:openai"
    prompt: PromptRef,        // library key, e.g. "solver" or "athz-03"
    tools: ToolScope,         // adi-mcp feature/tool selection (see §7)
    model: Option<String>,    // backend-specific alias, e.g. "opus", "gpt-5-codex"
    params: AgentParams,      // temperature, max turns, permission mode, extra env
    role: Option<String>,     // "solver" | "planner" | "triager" | …  (drives task→agent map)
    tags: Vec<String>,        // free-form (e.g. milestone tag) for dispatch/filtering
    starred: bool,            // pinned in the UI / preferred for quick-dispatch
}
```

`AgentDef`s persist under an `adi-config` module (e.g. `~/.adi/mono/agents/<project>/<name>.toml`)
or inside the project dir. They're created via CLI/API/UI and seeded in bulk by the roster
(§9). A `backend` value is parsed to a `BackendRef { kind: Cli|Api, engine: String }`.

Hooks are **not** a field on `AgentDef`. Lifecycle behaviour lives in one **global,
event-driven hooks registry** — see §9a. An "agent pre/post hook" is just a global hook bound to
the `agent.pre_run` / `agent.post_run` events and filtered to that agent.

---

## 6. Backend contract (CLI **and** API)

The core abstraction the reference app lacks. One trait; two impl styles selected per agent.

```rust
// Illustrative.
#[async_trait]
trait AgentBackend {
    /// Launch the agent for one run and return a handle that streams structured events.
    async fn spawn(&self, req: SpawnRequest) -> Result<Box<dyn AgentSession>>;
    /// Which tool-transport this backend consumes (see §7): native MCP config vs. in-loop client.
    fn tool_transport(&self) -> ToolTransport;
    /// Human/UX metadata (display name, whether it needs a TTY, supported models).
    fn caps(&self) -> BackendCaps;
}

struct SpawnRequest {
    cwd: PathBuf,                 // the adi-fs workspace (project) dir
    system_prompt: String,        // composed prompt (§8)
    initial_message: Option<String>, // the runtime "task" line
    model: Option<String>,
    tool_config: ToolConfig,      // adi-mcp endpoint + selected features/tools (§7)
    env: BTreeMap<String,String>, // scoped context (project id, webhook url, api keys)
    params: AgentParams,
}

#[async_trait]
trait AgentSession {
    /// Structured events — the replacement for TUI capture-pane scraping.
    fn events(&mut self) -> impl Stream<Item = AgentEvent>;  // Output, Status, ToolCall, Usage, Exit
    async fn send(&mut self, message: &str) -> Result<()>;   // follow-up turn
    async fn interrupt(&mut self) -> Result<()>;
    async fn kill(&mut self) -> Result<()>;
    fn status(&self) -> AgentStatus;                         // starting|working|waiting|idle|done|failed|killed
}
```

### 6a. `CliBackend` (codex, claude)
Shells out to the vendor CLI. **Prefer each vendor's headless/streaming mode over a TTY**, so
we get structured events and a real exit code instead of scraping a terminal:

- **claude:** `claude -p <prompt> --output-format stream-json …` (non-interactive, JSON event
  stream) rather than typing into the interactive TUI.
- **codex:** `codex exec …` (non-interactive) rather than the interactive UI.

Only fall back to a PTY (via `portable-pty`) for a backend that has *no* headless mode; even
then the parser is the backend's problem, isolated behind `AgentSession`, so the racy
send-keys/gate/double-Enter dance from the reference app disappears from the orchestrator.
Activity/`status` comes from the event stream (a `Status` event or inactivity timer), **not**
from grepping `esc to interrupt` / `❯` out of a pane.

### 6b. `ApiBackend` (anthropic, openai)
An in-process agent loop the orchestrator owns: messages → provider API → tool calls →
results → repeat, until stop. Tool calls are dispatched to `adi-mcp` via an MCP **client**
(`rmcp`), so the *same* tool surface backs both styles (§7). Emits the same `AgentEvent`s.

### 6c. Why both
CLI backends give us today's coding agents (codex/claude) with their own harnesses for free;
API backends give full control (custom loops, judges, cheaper models) where a CLI doesn't fit.
The contract is the same, so the orchestrator, prompts, tools, and lifecycle are
backend-agnostic — the single most important improvement over the reference app.

---

## 7. Tool surface via `adi-mcp` (replacing PATH shims)

The reference app exposes `bin/` tools by writing shims onto `PATH` and pasting `help-llm`
docs into the prompt. We already built the better mechanism: **`adi-mcp` over stdio, with tools
selected per launch via `--features "tasks[create,list],files[read],…"`** (feature- and
tool-level scoping shipped in `adi-mcp`).

- **`ToolScope`** on an `AgentDef` is exactly an `adi-mcp` feature/tool selection string.
- **CLI backends** are launched with the vendor's MCP config pointed at
  `adi-mcp --features "<scope>"` (e.g. claude `--mcp-config`, codex's MCP config). The agent
  discovers tools + schemas via MCP `tools/list` — no `help-llm` injection, no shim dir.
- **API backends** run `adi-mcp` (or link it) as an MCP client and expose the same tools in the
  loop.
- **Scoping** is enforced two ways: `adi-mcp` already gates its `files` tools to a *registered*
  `adi-project` and jails them with `adi-fs`; and the `ToolScope` limits *which* tools exist at
  all. So an agent physically cannot touch another project's files or call a tool it wasn't
  granted.

**Conversion of the `bin/` tools:** the reference CLIs (`target`, `task`, `finding`,
`functionality`, `graph`, `notes`, `milestone`, `links`, `bundle`, `cdp`, `webhook`) become
`adi-mcp` **features** over time. `adi-mcp` already ships `tasks`/`projects`/`files`/`status`;
the domain-specific ones (graph, bundle, cdp, finding, …) are added as new `adi-mcp` features
when needed. `adi-agents` doesn't care which exist — it just passes a scope string.

### 7a. Global CLI store — a shared, agent-editable toolbox

Compiled `adi-mcp` features are the *stable* tools. Agents also need a **dynamic** tool layer
they can grow themselves — the generalization of the reference app's `bin/` directory (a
**global** store of executables every agent draws from) plus its `help-llm` convention. This is
a first-class concept in `adi-agents`.

- **Global storage of CLIs.** One **shared** registry of CLI *tools* (scripts/executables),
  **not** per-agent — authored once, available to every agent (optionally split into a
  workspace-global store and a per-project store). Backed by `adi-config` (the index/metadata)
  and an `adi-fs`-jailed directory (the scripts themselves).
- **`help-llm` is mandatory metadata.** Every CLI implements a `help-llm` subcommand that prints
  its LLM-facing usage doc (verified: *every* reference `bin/` CLI does this, and
  `cliCommandHelp()` runs `<exe> help-llm`). The store indexes it, so a tool is
  **self-documenting** — the model learns that a tool exists and how to drive it *from the tool
  itself*, not from hand-written prompt text.
- **Agent-editable.** An agent can **author and edit** CLIs in the store: `clis_create <name>
  <source> --help-llm …`, `clis_edit <name> …`, alongside `clis_run <name> [args]`,
  `clis_list`, `clis_show`. The toolbox is therefore **self-extending** — an agent that writes a
  useful script leaves it in the store for every other agent, and can revise an existing tool
  (fix a bug, improve its `help-llm`).
- **Attach to other agents.** Because a CLI lives in the shared store and carries its own
  `help-llm`, granting it to another agent is just adding its name to that agent's `ToolScope`
  — **the LLM help travels with it**, so the receiving agent gets a fully *documented* tool for
  free. This is the reference app's per-agent `cliCommands` allow-list, made dynamic and
  cross-agent (one agent's tool becomes another's, no prompt edits).
- **Exposed through `adi-mcp`.** The store is one `adi-mcp` feature (e.g. `clis`) whose tools
  run / list / show / create / edit store CLIs. A `ToolScope` selects both *which store CLIs* an
  agent may run and *whether* it may author them, e.g. `clis[run:graph,notes,finding]` (use
  three) vs `clis[list,run,create,edit]` (a "tool-smith" agent). Discovery stays MCP
  `tools/list`, so a store CLI reaches the agent over the same transport as a compiled feature.

**Two tiers, one surface.** Stable/security-sensitive tools are compiled `adi-mcp` features;
experimental or agent-authored tools live in the CLI store. Both reach the agent over the same
MCP channel, so a backend never knows the difference — and a proven store CLI can later be
"promoted" into a compiled feature.

**Safety (see §12).** Agent-authored code that executes is privileged: store writes are
`adi-fs`-jailed and allow-listed, execution runs under the agent's permission mode, and the
authoring capability (`create`/`edit`) is a `ToolScope` grant given only to trusted/tool-smith
agents — optionally behind an operator-approval step before a newly authored CLI becomes
runnable by others.

---

## 8. Prompt library

Converts the reference app's `prompts/<name>.md` + `promptTemplate` + `milestone → agent →
prompt` mapping into a general, backend-agnostic library.

- **`PromptLibrary`** is a keyed collection of prompt files. Default location: an `adi-config`
  module or a project-local `prompts/` dir. Keys match `^[a-z0-9-]+$` (same rule as the
  reference app). API: `list()`, `get(key)`, `path(key)`, `save(key, body)`, `delete(key)`.
- **Resolution.** An `AgentDef` names a `PromptRef` (an explicit key, or a `role`+`tag` the
  library resolves — the generalization of `comboAgentFor(milestone)`: e.g. tag `athz-03` →
  key `athz-03`; bare role `solver` for tag `athz` → key `athz-solver`).
- **System-prompt composition** (`build_system_prompt`): `preamble` (workspace + posture +
  absolute-path facts) **+** the resolved prompt body **+** *optionally* a generated tool
  section. Because tools now come from `adi-mcp` (`tools/list`), the tool section can be
  dropped or reduced to a one-line pointer — the agent introspects tools over MCP.
- **Runtime message.** The "task" line (the reference app's `taskPrompt(task)`) stays *separate*
  from the system prompt and is passed as `SpawnRequest.initial_message`.
- **Prompt content is out of scope** (see §1). We ship the format + a minimal example
  (`solver`, `planner`, `triager`); real collections are authored per project.

---

## 9. Lifecycle & orchestration

Keep the reference app's proven loops; drive them off structured status.

- **Create → spawn → supervise → reap**, all through the `Orchestrator`.
- **Concurrency.** Per-project **slots** (cap, default like the app's 10) and a global **capacity**
  for the auto-spawner (default 3), measured across all live `Session`s.
- **Auto-spawn ("task starter").** A periodic sweep drains the **task queue**: for each *ready*
  task that **resolves to an existing agent**, if under capacity, it emits `agent.pre_run` (§9a),
  spawns a `Session`, and marks the task in-progress. **Hard rule (ported):** never auto-start a
  task that is blocked, already in-progress, or done. The queue is the `adi-mcp` `tasks` feature
  (see §14).
  **Assignment resolution — how a task picks its agent (first match wins):**
  1. **Explicit assignee** — the task names an agent → that agent.
  2. **Tag == agent name** — the task's tag equals an `AgentDef` name → **auto-assigned, zero
     config**. Tag a task `athz-03` and the `athz-03` agent picks it up automatically.
  3. **Manual map** — a configurable `tag → agent` table for tags that *don't* match a name
     (e.g. `athz → athz-solver`). This is the reference app's `MILESTONE_AGENT_MAP` /
     `comboAgentFor`, kept for the "previous version" behaviour.
  4. **Unresolved** → the task stays unassigned for an operator to map or assign by hand.
- **Reap.** A periodic sweep kills a `Session` past a grace age whose idle time exceeds a
  threshold. **Idle is derived from the backend event stream** (time since last `Output`/
  `Status`), not from a log's mtime; "waiting for input" counts as idle; a `working` session
  with no output past a `stuck` threshold is flagged. Exit is a real `Exit` event now.
- **Starred / pinned** `AgentDef`s for one-click dispatch (ported).
- **Roster.** `roster::seed(project)` idempotently creates a project's canonical `AgentDef`s
  from a roster spec (the generalization of the reference app's ~130-agent `agent-roster.json`).
  The roster is *data*, not code — a project supplies its own.
- **Supervision.** The orchestrator can run inside a host process, or as an `adi-hive`-supervised
  service (the workspace already supervises long-running services this way). Boot-time
  auto-start/reaper toggles persist via `adi-config` (the app's `automation-state.json`).

### 9a. Lifecycle hooks (global, event-driven)

Hooks live in **one global registry**, not on individual agents, and fire on **lifecycle
events**. The old per-agent pre/post scripts are just two events (`agent.pre_run` /
`agent.post_run`) any hook can subscribe to and filter to a specific agent — so the capability is
preserved and generalized: *any* event can trigger *any* number of hooks.

**Triggers — a catalog of lifecycle events** the orchestrator, task queue, and sessions emit
(extensible: new subsystems add events; hooks stay decoupled from the emitters):
- **Task:** `task.created`, `task.ready`, `task.blocked`, `task.assigned`, `task.completed`,
  `task.archived`, `task.reopened`.
- **Agent / session:** `agent.pre_run` (before the backend starts), `agent.spawned`,
  `agent.post_run` (after exit), `agent.failed`, `agent.stuck`, `session.reaped`.
- **Orchestrator:** `roster.seeded`, `sweep.tick`, … and anything a future subsystem emits.

**A hook** binds one-or-more triggers to a **run template**, with an optional match filter:
```toml
[[hook]]
name     = "notify-done"
on       = ["task.completed", "agent.failed"]      # a hook can watch many triggers
match    = { project = "acme", tag = "athz-*" }    # optional; omit = fire for all
template = "notify"                                 # a reusable run template (below)
blocking = false                                    # fire-and-forget
```

**Custom run templates** — *how* a hook runs is a reusable, parameterized template (`sh` by
default) with the event context interpolated, so many hooks share one runner and you can author
bespoke ones per event:
```toml
[template.notify]
body = "curl -s $ADI_WEBHOOK -d \"{{event}} {{task.id}} {{task.title}}\""

[template.warm-browser]
body = "./.adi/hooks/warm-browser.sh"    # a workspace .sh, resolved through the adi-fs jail
```

**Per-event customization + opt-out.** A trigger has **no built-in lifecycle** of its own — if no
hook subscribes, nothing runs. You attach a custom template for a specific event, override it, or
**disable a hook for a given event** (`enabled = false`, or a `skip` match) — "no lifecycle for
this event." The system is opt-in per trigger, never a fixed pre/post pair.

**Execution contract.**
- **Blocking vs async.** A hook declares `blocking`. `agent.pre_run` hooks are blocking and a
  **non-zero exit aborts the run** (the old pre-hook guarantee); notify-style hooks
  (`task.completed`, …) are async fire-and-forget. `agent.post_run` runs on success *or* failure
  and receives the outcome.
- **Context.** Each hook runs `sh`-style under a timeout, cwd = the relevant `adi-fs` workspace,
  with the event payload in the environment — `ADI_EVENT` plus whichever of `ADI_TASK`,
  `ADI_AGENT`, `ADI_SESSION`, `ADI_PROJECT`, `ADI_WORKSPACE`, `ADI_EXIT_STATUS` the event carries
  (the same fields are available as `{{…}}` to templates).
- **Storage.** The hook registry + templates persist via `adi-config`; script bodies a template
  references resolve through the workspace jail, so they version with the project.

---

## 10. Persistence, sessions & scope

- **AgentDefs** and **orchestrator prefs** persist via `adi-config` (typed TOML / raw JSON),
  not a bespoke SQLite schema. **Targets** are `adi-projects`.
- **Workspace scope.** Each `Session` runs with `cwd` = the project dir and is confined by an
  `adi-fs` `Jail`; injected `env` carries the project id + non-secret context; secrets come
  from a config source, never baked into an `AgentDef`.
- **Sessions/transcripts.** Two concepts (as in the reference app, but explicit): a **run**
  (a live `Session`) and its **transcript** (the persisted event/message log). Transcripts
  store under the project's config dir for audit/replay. An **audit checkpoint** mechanism
  (a `since` timestamp + reason, as the app's `session-audit.json`) marks transcripts to
  ignore before a fix.

---

## 11. Control surface

- **CLI (`adi-agents`)** — `create`, `list`, `run <agent>`, `stop <session>`, `ps`, `logs
  <session>`, `roster seed`, `auto on|off`, `reap on|off`. Thin adapter over the orchestrator,
  in the style of `adi-mono`.
- **API / UI** — later, surface running sessions + operator toggles in `adi-app` (the workspace
  already has a control-panel pattern), replacing the reference app's Next.js Home dashboard.
- **Terminal attach** — for CLI backends, optionally expose the underlying process for a live
  view (the reference app used `ttyd`); with headless backends this becomes a structured
  event tail instead of a raw TTY (safer — the app's `ttyd` bound all interfaces).

---

## 12. Security & posture

- **Scoped by construction:** `ToolScope` + `adi-mcp`'s registered-project gate + `adi-fs` jail
  mean an agent only sees the tools it was granted and only its project's files.
- **Agent-authored tools are privileged (§7a):** creating/editing CLIs in the global store is a
  `ToolScope` capability granted only to trusted/tool-smith agents; store writes are
  `adi-fs`-jailed and allow-listed; and a newly authored CLI can require operator approval
  before it becomes runnable by other agents (arbitrary agent-written code executing is a real
  risk surface — gate it deliberately).
- **Permissions:** the reference app runs most agents `--dangerously-skip-permissions`. In
  `adi-agents`, permission mode is an explicit `AgentParams` field with a **safe default**;
  bypass is opt-in per `AgentDef`.
- **Secrets** are injected from a config source at spawn, never stored in an `AgentDef` or a
  transcript.
- **No raw shell on the network:** prefer headless event tails over a TTY bound to all
  interfaces.
- Respects this workspace's standing rules (never disturb protected services; pick free ports;
  see repo `CLAUDE.md`).

---

## 13. Conversion map (reference app → adi-agents)

| Reference app | adi-agents |
|---|---|
| `claude` hardcoded (`CLAUDE_BIN`) | `AgentBackend` trait; `cli:claude`, `cli:codex`, `api:anthropic`, `api:openai` |
| `tmux` + `send-keys`/`capture-pane` + `ttyd` | headless CLI (`claude -p --output-format stream-json`, `codex exec`) or API loop; structured `AgentEvent`s; PTY only as fallback |
| shell-ready gate / double-Enter / sentinel bounce | gone (no TUI typing) |
| `prompts/<name>.md` + `promptTemplate` + `--append-system-prompt-file` | `PromptLibrary` + `PromptRef` + `build_system_prompt` |
| `milestone-agent-map.ts` (`comboAgentFor`) | `PromptRef` role+tag resolution |
| `bin/` CLIs via PATH shims + `help-llm` in prompt | `adi-mcp` over stdio, `ToolScope` = `--features` selection; tools discovered via `tools/list` |
| `bin/` as a static global executable dir + `help-llm` convention + static roster `cliCommands` | **global, agent-editable CLI store** (`adi-mcp` `clis` feature) with `help-llm` metadata; tools authored/edited by agents and attached across agents via `ToolScope` (§7a) |
| per-agent `cliCommands` allow-list + separate `mcpServers` list | one `ToolScope` per `AgentDef` (unifies both), with per-role presets (scanner/solver/planner) |
| env-only scoping (`TARGET_SLUG`/`TARGET_DIR`) | `adi-projects` + `adi-fs` jail + injected context |
| 1 SQLite table + flat JSON | `adi-config` + `adi-projects` |
| targets | `adi-projects` |
| task queue → auto-spawn | `adi-mcp` `tasks` feature → orchestrator auto-spawn |
| reaper via log mtime + pane grep | reaper via backend event-stream idle time + `Exit` |
| `agent-roster.json` (~130 agents) | `roster::seed` from a project-supplied roster spec |
| `instrumentation.ts` boot loops + `automation-state.json` | orchestrator loops + `adi-config` prefs; optional `adi-hive` service |
| Next.js Home dashboard / `ttyd` modal | `adi-app` UI + structured event tail (later) |

---

## 14. Integration with the current adi stack (and rollout)

`adi-agents` is not a new silo — it wires together primitives this workspace already has.

**Where each piece plugs in:**
- **Task queue → `adi-mcp` `tasks` feature.** The task starter (§9) reads/writes tasks through
  adi-mcp. **Current state:** the `tasks` feature *exists* but **no tasks are populated yet** — so
  step one is simply to start creating tasks (by hand, from the UI, or by planner agents); agents
  come after. Two small extensions the queue needs to drive assignment + the hard rule: a **`tag`**
  (and/or an explicit **`assignee`**) field on a task, and a **`blocked`** status (adi-mcp tasks
  today are pending/in_progress/done/cancelled — read pending as "ready", in_progress as "doing";
  add `blocked` so the hard rule has something to skip).
- **Targets → `adi-projects`.** An agent is scoped to a project; the roster is seeded per project.
- **Tool surface → `adi-mcp`.** `ToolScope` = an `adi-mcp --features` selection (§7), including the
  agent-editable CLI store (§7a).
- **Workspace → `adi-fs`.** The session cwd is an `adi-fs` jail; lifecycle-hook scripts (§9a) and
  the CLI store write inside it.
- **State → `adi-config`.** AgentDefs, roster, orchestrator prefs, and transcripts.
- **Supervision → `adi-hive`.** The orchestrator can run as an `adi-hive`-supervised service (like
  the other long-running services), reachable behind the front door.
- **Operator surface → `adi-core` CLI + `adi-app` UI.** `adi-agents` subcommands for scripts;
  running sessions + the auto-spawn/reap toggles surface in the app control panel.

**Rollout order (matches "tasks first, agents soon"):**
1. **Start using tasks.** Populate the `adi-mcp` `tasks` queue and add the `tag`/`assignee` +
   `blocked` extensions. No agents yet — this stage is useful on its own.
2. **MVP agent.** One CLI backend (claude headless) a human runs against a task (§15).
3. **Auto-spawn.** The task starter picks up tasks by tag==name / manual map, firing lifecycle
   hooks (§9a) around each run.
4. **Roster, reaper, CLI store, UI.** The full lifecycle and operator surface.

---

## 15. Phased plan

1. **MVP — one backend, real spawn.** `crates/adi-agents` skeleton; `AgentBackend` trait; **one**
   `CliBackend` (claude headless, `-p --output-format stream-json`); `AgentDef` + `store`;
   `PromptLibrary`; launch with `adi-mcp --features <scope>` as the tool surface; `adi-agents
   run <name>` streams events to the terminal. *Proves the contract end-to-end.*
2. **Second backend.** Add `cli:codex` (`codex exec`) — validates the abstraction. Then one
   `ApiBackend` (`api:anthropic`) with the in-loop MCP client — validates CLI+API parity.
3. **Orchestrator.** Slots + capacity; auto-spawn off the `adi-mcp` `tasks` queue; reaper off
   the event stream; starred; roster seeding; persisted auto/reap toggles.
4. **Domain features.** Port the remaining `bin/` tools into `adi-mcp` features as needed
   (graph, bundle, cdp, finding, …); author real prompt collections per project.
5. **Surfaces.** `adi-app` UI + `adi-hive`-supervised service; event-tail terminal view.

---

## 16. Open questions

- **PTY dependency.** Do our target CLI backends (codex/claude) have headless modes rich enough
  that we can avoid a PTY entirely? (Strongly preferred — kills the whole class of TUI races.)
  If not, which PTY crate (`portable-pty`?) and where is its parser isolated?
- **`adi-mcp` link vs. spawn.** For `ApiBackend`, do we spawn `adi-mcp` as a child (stdio, clean
  isolation) or link it as a library (fewer processes)? Leaning: spawn, for uniformity with CLI
  backends.
- **Where AgentDefs/prompts live** — a central `adi-config` module vs. inside each project dir.
  Leaning: project-local for prompts+roster (portable with the project), config module for
  orchestrator prefs.
- **Task-queue source** — is `adi-mcp`'s `tasks` feature the canonical queue for auto-spawn, or
  do we allow a pluggable queue? Leaning: `tasks` is the default; keep the queue behind a small
  trait.
- **Model/param taxonomy** — a shared alias set vs. per-backend passthrough. Leaning:
  per-backend passthrough with a thin shared layer for common knobs (model, temperature, max
  turns, permission mode).
- **Multi-agent patterns** — do we bake in planner→solver→triager pipelines (the reference
  app's `-task-creator`/`-solver`/triager roles), or leave orchestration of *cohorts* to a
  higher layer? Leaning: `adi-agents` provides single-agent lifecycle + the auto-spawn queue;
  cohort choreography is a thin layer on top (and could itself be a workflow).
