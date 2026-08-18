# Agent runner refactor — spec

The design subagents implement against. Read this before touching `crates/adi-agents`.

## The problem

`Backend` is a closed enum and every capability is a `match` over it. Fourteen dispatch
functions in `run.rs`, plus `arguments::validate_builtin`, `progress::parse`,
`progress::capabilities`, and `tool_help::applies_to` — each one a separate place that has
to learn about every backend. Worse, the *layering* is wrong: session concerns (a run's
`hidden` flag, its queue, its transcript) are routed through the backend, because run
storage is keyed by executor subdir (`sessions/<process|harness>/<agent>/…`).

Two consequences that are bugs, not just ugliness:

- Change an agent's `backend` and its entire run history disappears. `runs_in`
  (`run.rs:135`) dispatches on the *current* backend, so it reads `sessions/harness/…` and
  never looks at `sessions/process/…`. Those runs can't be listed, peeked, stopped, or
  deleted.
- `stop` sends TERM, waits 500ms, and never escalates (`detached.rs:283`). A CLI that
  ignores TERM cannot be killed.

## The principle

**A runner is a process manager. It is not a place to keep information.**

Anything durable about a session — what it's called, when it started, whether it's hidden,
what's queued behind it, what it said — belongs to the session store. The runner starts
things, reports whether they're alive, stops them, and translates its own engine's output
into normalized events. Nothing else.

## Three layers

```
Agents (agent layer)   resolves + materializes: cwd, .bin, PATH, env  →  RunSpec
      │
SessionStore           the record, the queue, hidden, transcript, the log
      │                                     ↑ borrowed view: &dyn Session
Runner                 start / alive / stop / events
```

The runner never sees `sessions_dir`, a `run_id`, a `hidden` flag, or a queue. The store
never parses an engine's wire format.

## Runner

```rust
pub struct RunnerKind(pub String);   // open, like Backend::Other — third parties mint their own

pub trait Runner: Send + Sync {
    fn kind(&self) -> RunnerKind;

    /// Can this spec run at all — args parse, engine configured. No side effects.
    fn check(&self, spec: &RunSpec) -> Result<()>;

    /// The only start verb. If the session already exists it is resumed; if not it is
    /// established. The caller NEVER chooses between the two.
    fn send(&self, spec: &RunSpec, session: &dyn Session, message: &str) -> Result<()>;

    fn is_alive(&self, session: &dyn Session) -> bool;

    /// Cooperative stop, escalating to a forced kill once `grace` elapses.
    /// `grace == ZERO` is an immediate force.
    fn stop(&self, session: &dyn Session, grace: Duration) -> Result<Stopped>;

    /// Which event kinds this runner can ever emit — the capability descriptor.
    fn emits(&self) -> EventKinds;

    /// Whether a later `send` continues the same thread rather than starting an unrelated
    /// one. Data, not an attempt: a reader decides whether to draw a reply box before
    /// anybody types in it. Defaults to `false` — a one-shot engine really does forget.
    fn resumes(&self) -> bool { false }

    /// Normalized events after `cursor`, plus the cursor to resume from.
    fn events(&self, session: &dyn Session, cursor: Option<&RawValue>) -> Result<EventBatch>;

    /// Typed extension. Terminal-family runners return `Some`; everyone else inherits `None`.
    fn as_terminal(&self) -> Option<&dyn Terminal> { None }
}

pub struct Stopped { pub was_running: bool, pub forced: bool }
pub struct EventBatch { pub events: Vec<RunEvent>, pub cursor: Box<RawValue> }

pub trait Terminal {
    fn send_keys(&self, session: &dyn Session, text: &str, key: &str) -> Result<()>;
    fn capture(&self, session: &dyn Session) -> Option<String>;
}
```

Rules that fall out, and must be respected:

- **No `Fresh`/`Resume` variant anywhere in the API.** Continuation is derived inside the
  runner from `session.has_started()`. `Continuation` (`conversation.rs:88`) becomes a
  private detail of the runner that needs it.
- **No `Handle` in any signature.** A runner that needs a local token keeps it in its own
  state slot (below). A runner that needs none — an API-backed runner whose liveness is a
  `GET /runs/{id}` — writes nothing.
- **`send` must read `has_started()` before it creates the log.** The store answers
  `has_started` by whether the log exists (not by the state slot, so an API-backed runner
  that stores nothing still resumes correctly). A runner that opens its log first and
  decides continuation second will try to resume a session it never established, on the
  very first turn. Build the command, then spawn.
- **No separate `kill`.** `stop(s, Duration::ZERO)` is the force path. A `kill()`
  convenience wrapper at a call site is fine; a trait method is not.
- **Never match on `kind()` to reach behaviour.** `kind()` is for labels, telemetry, and
  the session record. Behaviour goes through `as_terminal()` and friends, so `pty`,
  `tmux-pty`, and a future `cloud-pty` need zero call-site changes.

## Session — a borrowed view, zero owned state

```rust
pub trait Session {
    fn id(&self) -> &str;
    fn agent(&self) -> &str;
    /// Whether at least one turn has run under this session. Drives resume-vs-establish.
    fn has_started(&self) -> bool;
    /// Runner-private, store-opaque. JSON.
    fn state(&self) -> Option<&RawValue>;
    fn set_state(&self, value: Box<RawValue>) -> Result<()>;
    /// The log the store owns. Must be a real path — a spawned child needs an fd.
    fn log_path(&self) -> &Path;
}
```

This is a *view over the store*, holding only ids and a reference. It must own no cached
state: the CLI, the daemon, and triggers all launch runs independently (`run.rs:117`), so
any in-memory copy is a second truth another process can invalidate.

The state slot is where per-runner detail lives, and the store never looks inside:

- detached → `{"pid": 4711, "session_id": "…"}`
- pty → `{"session": "adi-agent-solver"}`
- api → nothing at all

## RunSpec — materialized context, passed down

The agent layer resolves and **materializes** everything before the runner is called. The
runner does no resolution, no `mkdir`, no tool sync. This already exists as
`launch_context` (`lib.rs:590`); it becomes the formal boundary.

```rust
pub struct RunSpec {
    pub cwd: PathBuf,                    // resolved, existing, PINNED PER SESSION
    pub path: String,                    // .bin + declared dirs + system dirs, assembled
    pub env: Vec<(String, String)>,      // secrets + db pointer + workspace vars + manifest env
    pub arguments: Box<RawValue>,        // the engine's own config, uninterpreted by the store
    pub tools: Vec<ToolHelp>,            // as DATA — the runner composes its own prompt
    pub system_prompt: Option<String>,   // the user's own, unmodified
}
```

Two constraints that must survive the refactor:

- **`cwd` is resolved once, at session creation, and re-used for every later turn.** The
  engine's session store is keyed by cwd; re-resolving on turn 5 makes the session
  unresumable and puts earlier turns' files out of reach (`harness/mod.rs:115-118`).
- **Spec assembly has side effects** (`sync_agent_bin` writes shims), so it belongs on the
  write path only. Never build a spec from a read/poll path — that is what `can_advance`
  exists to avoid (`harness/mod.rs:124-126`).

## Pre-run — the tool calls the launcher already knows

A launcher usually knows the agent's first move. The bug-bounty filer has just written the task, so
it holds the id; a target agent always opens by reading its brief. Left to the agent that costs a
whole round trip: the model reads the instruction, emits one tool call, and the turn ends having
learned only what the launcher could have said.

So a launch may **really run** commands before the opening message goes out — `crate::prelude`:

```
agent manifest `prelude = [...]`      standing orders, every run of this agent
LaunchOptions::pre_run = [...]        this run's own, after them
```

Same shell as the agent's `Bash` (so a `cd` or an `export` carries into its first call), same cwd,
same `PATH` and environment — including the `.bin` shims, which exist on no other `PATH`. Real exit
status, real stdout and stderr, truncated the way a `Bash` result is. Nothing is predicted or
summarized, and a command that fails or cannot start says so rather than being dropped.

What lands where:

| | |
|---|---|
| the engine's message | the words, then a `# Already run for you` block of `<pre-run command="…" status="ok\|failed">` |
| the transcript's opening turn | the words **unchanged**, plus one `Step::Tool` per command |

That split is the one `for_engine` already makes for an image's file paths: what the engine is told
is not what was said. It also means a reader sees the calls where the agent's own calls appear
instead of a wall of quoted output wedged into the message.

**It lives at the agent layer, not in a runner** — every engine inherits it, and the runner still
knows nothing but "here is the message to send". The one engine that needs help is `harness:adi`,
which is handed a conversation id rather than a message and replays the stored transcript: it
rebuilds the identical block from the turn's steps (`prelude::block_of_steps`). That round trip is
lossless because a step carries the command, the output, and the status — which is why the exit code
lives *inside* the output, spelled `(exit 7)` exactly as the `Bash` tool spells it.

A terminal backend runs no prelude: nothing has been typed into it yet, so there is no message for
the output to arrive on.

## Events — pull with a cursor, normalized by the runner

Reuses the existing normalized types (`Step`, `ToolStatus`, `TurnMetrics`) — do not invent
parallel ones.

```rust
pub enum RunEvent {
    Step(Step),                                  // Message / Thinking / Tool
    Answer { text: String },                     // the turn's final message
    Metrics(TurnMetrics),
    Finished { ok: bool, error: Option<String> },
}

pub struct EventKinds { pub message: bool, pub tool_call: bool, pub thinking: bool, pub metrics: bool }
```

**Pull, not push.** A push sink only works inside the process that spawned the run; this
platform supports runs launched by the CLI or a trigger and viewed in the app. Pull with an
opaque cursor works from any process and survives a daemon restart. The cursor is minted by
the runner — a byte offset for a log-backed runner, an event id for an API one.

The store persists the raw log; the runner turns bytes → events lazily on read (memoize —
`memo::parsed_log` already has this shape). `progress::parse`'s backend match dissolves
into each runner parsing its own format.

**Terminals opt out.** A pty can't produce messages and a screen is a snapshot, not an
append stream. A terminal runner declares an empty `emits()` and is read through
`Terminal::capture()`.

`BackendCapabilities` (`progress.rs:96`) stops being hand-maintained: `tool_steps`,
`thinking`, `metrics` derive from `emits()`.

## What dies — all of it, as of stage 5

| was | became |
|---|---|
| 14 `match` arms in `run.rs` | store methods + one runner lookup by kind |
| `progress::parse` match | each runner parses the formats that are its own |
| `progress::capabilities` matrix | derived from `emits()` + `as_terminal()` + `resumes()` |
| `tool_help::applies_to` + `fold_into` | each runner composes its own prompt from `spec.tools` |
| `decorated()` + re-validate fallback | gone — the store stops editing engine config |
| `sessions/<subdir>/<agent>/…` layout | records keyed by session id, `backend` as a field |
| `backends/{process,pty,harness}/mod.rs` wrappers | argv builders only; the lifecycle is the runner |
| `backends/harness/conversation.rs` (849 lines) | `store/` — `Turn` moved to `store::transcript` |
| the run half of `backends/detached.rs` | `store/`; only the spawn/pid/signal primitives remain |

Two things survived that look like they should not have, and the reason matters:

- **`arguments::validate_builtin` stays.** Not dispatch — `save` calls it, so a bad manifest is
  refused when it is *written* rather than at launch. Each runner's `check()` parses its own typed
  arguments independently; that is the launch-time gate.
- **`Agents::run_adi_turn` stays a direct call**, not a runner verb. Every other engine answers a
  turn in a child the runner spawned and watches; the `adi` loop *is* that child. Its prompt still
  comes from the runner, exported as `ADI_SYSTEM_PROMPT` — so there is exactly one composition
  path, which is the point.

## Staging

1. ✅ **Types** — `runner/`: `Runner`, `Session`, `RunSpec`, `RunEvent`, `Terminal`,
   `RunnerKind`, plus `registry::runner_for` — the single place a `Backend` becomes
   behaviour.
2. ✅ **Store** — `store/`: `SessionStore`, `SessionRecord`, `SessionRef`, queue,
   transcript, and `migrate_legacy` off the old `<sessions>/{process,harness}/…` layout.
3. ✅ **Runners** — `DetachedRunner` (one per engine) and `PtyRunner`, each parsing its own
   args, composing its own prompt, and emitting its own events.
4. ✅ **Rewire** — `Agents` talks to store + runners.
5. ✅ **Cleanup** — the table above, deleted once nothing called it.

The public API of `adi-agents` did **not** change: `adi-app`, `adi-cli`, and `adi-webapp-api`
have ~20 call sites between them and compile untouched. Only the insides moved.
`cargo check -p adi-app -p adi-cli -p adi-webapp-api` with those crates unedited is the
proof, and `git status` showing no file outside `crates/adi-agents/` is the other half.

### Landed

`cargo check -p adi-agents --all-targets` is warning-free: no `#[allow(dead_code)]` is left
anywhere in the crate, which is the actual check that the old path is gone rather than merely
unreferenced. 185 tests pass.

Nineteen tests were deleted with the code they covered. Each rule they guarded was re-asserted
at the layer that now owns it — the two that nearly slipped through were **stopping a run clears
its queue** (`lib.rs`, was `conversation::stopping_an_answer_forgets_what_was_queued_behind_it`)
and **a `working_dir` never reaches the engine's argv** (`runner::detached`, was
`harness::a_working_dir_never_reaches_the_turn_command`). When deleting a test, find its
replacement first.

`run_of_an_unrunnable_backend_is_400` (`adi-webapp-api`) fails in this environment before and
after these changes, identically. It is not caused by this work.

## House rules

- Never run `cargo fmt` in this repo — the tree is not rustfmt-clean and it rewrites ~64
  untouched files.
- `cargo build -p adi-agents` and `cargo test -p adi-agents` are the gates.
  `run_of_an_unrunnable_backend_is_400` fails in this environment for unrelated reasons.
- Comment density and idiom: match the surrounding code. It explains *why*, not *what*.
