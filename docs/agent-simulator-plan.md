# Agent simulator — implementation plan

What to build, in what order, against which code. Read
[`agent-simulator-status.md`](./agent-simulator-status.md) first for what already exists and
what is broken, and [`agent-runner.md`](./agent-runner.md) for the runner/store contract this
must not violate.

## The rule the whole feature rests on

The simulator must never grow its own copy of anything a real run does. Not the prompt, not
the tool schemas, not the execution, not the transcript. Every place it duplicates instead of
calls is a place where the simulation shows something the agent does not actually see, which
is the one failure that makes the feature worthless.

Concretely: one composer, one tool table, one `run()`, one session store.

---

## Phase A — finish the components in `adi-ui`

In progress. Nothing here has compiled yet.

1. **Get it to build.** `cd crates/adi-ui && trunk serve` (port 9081). Run it as a direct
   foreground child, not with `&` — see the environment notes in the status doc. Fix whatever
   the compiler says about `tokens.rs`, `toolform.rs` and the two playground panels.
2. **Look at it.** Headless Chrome against `http://127.0.0.1:9081`; the dark theme needs an
   inline `color-scheme`, not `data-theme`. Check: a token carrying `\n` shows `⏎` *and*
   breaks; a token that is only `\n` still shows its colour; leading spaces stay attached to
   the following word; `ToolForm` gives `Bash` a wide `command`, a short `timeout_ms`, and a
   self-describing `background` checkbox.
3. **Add the three components the flow still needs**, same crate, same conventions:
   - `TurnBlocks` — what has been emitted into the open turn: one row per block (`text` or a
     named call), each droppable. The turn is open until it is ended, so this is the staging
     area, not history.
   - `StopLine` — how the last turn ended: `tool_use` (loop runs again) or `end_turn` (the
     run yields to the person), with the OpenAI spelling beside it. It is response metadata,
     not prompt text, so it must be visually outside the mono document.
   - `Simulator` — the shell that arranges prompt view, staged blocks, composer tabs
     (prose / tool call), the wire preview, the end-turn control, and the user-turn composer.
     Keep it presentational: it takes data and emits callbacks, holds no fetch logic.
4. **The flagging affordance.** Selecting any passage of the prompt offers "flag this"; flags
   collect with a note field. This is the point of the feature for the user — reading the
   prompt as the model and marking what is wrong with it — so it is not optional polish.
5. Add rows to the component table in `crates/adi-ui/README.md`.

---

## Phase B — the runner in `adi-agents`

### B1. One prompt composer

Composition currently lives in `crates/adi-agents/src/runner/detached.rs` as `pub(super)`
helpers: `own_prompt` (:524), `with_workspace` (:539), `with_knowledge` (:554),
`with_tool_help` (:576). They chain into the agent's own prompt plus `# Where you are`
(`workspace.rs:147`), `# What you know` (`knowledge.rs:117`), `# Your tools`
(`tool_help.rs:33`).

Move the chain into `crates/adi-agents/src/runner/prompt.rs` with one public entry:

```rust
pub fn compose(spec: &RunSpec, stored: Option<String>) -> Option<String>;
```

`detached.rs` calls it. The simulator calls it. Do not re-render it anywhere else — the
`tool_help` frozen-at-launch rule (`RunSpec::tool_help`) exists because re-deriving it mid
conversation silently changes the prompt between turns and throws away every cached token.

### B2. Tool declarations and execution, as public data

`crates/adi-agents/src/backends/harness/tools.rs` has `TOOLS: &[ToolSpec]` (name,
description, `schema: fn() -> Value`) and `run(name, input, ctx) -> Result<String, String>`
(:302). Both are `pub(crate)`.

Add a narrow public surface:

```rust
pub fn declarations() -> Vec<ToolDeclaration>;                    // name, description, schema
pub fn execute(name: &str, input: &Value, ctx: &Ctx<'_>) -> Result<String, String>;
```

The simulator must call `execute`, never reimplement a tool. That is what makes `Bash` share
the conversation's shell (`backends::shell`), makes `Ask` refuse under `unattended`, and makes
a failed call return the same text the model would have read.

### B3. `HumanRunner`

New `crates/adi-agents/src/runner/human.rs`, implementing `Runner` with
`RunnerKind("human")`:

- `check` — the spec resolves; there is nothing to launch.
- `send` — append the incoming user message to the transcript and mark the session as waiting
  on a human turn. No process is spawned; the "engine" is a person at a browser.
- `is_alive` — true while the session is open and not stopped.
- `stop` — close it. There is no signal to escalate to.
- `resumes` — `true`.
- `emits` — message and tool events.
- `events` — read from the transcript the store owns.

Keep the doc's principle: the runner is a process manager, not a place to keep information.
Everything durable goes in the session store.

### B4. Resolve the runner from the session, not the backend

`runner_for(&Backend)` (`runner/registry.rs:17`) picks a runner from the agent's *current*
backend. A simulated run is the same agent run by a different runner, so this cannot stay the
only lookup.

Record the runner kind on the session record (`store/record.rs`) at creation, and resolve:
session record first, `runner_for(backend)` only when there is no record. This also fixes the
bug `agent-runner.md` opens with — change an agent's backend today and `runs_in` looks in the
wrong directory and its history disappears.

### B5. Launch entry point

```rust
impl Agents {
    pub fn simulate(&self, agent: &str, message: &str) -> Result<RunId>;
}
```

It must build the `RunSpec` through the **same** path a real launch uses (`launch.rs`: cwd,
`.bin`, PATH, env, secrets, knowledge, workspace vars), then create the session with
`HumanRunner` and a `simulated` flag on the record. Always a fresh run — continuing an
existing transcript is explicitly out of scope.

Consequences that should fall out for free, and are worth asserting in tests: the run appears
in `agents runs`, keeps a transcript, records an outcome, and publishes
`adi.agents.run.finished`.

---

## Phase C — API in `adi-webapp-api`

Handlers live in `crates/adi-webapp-api/src/handlers/agents.rs`, wire types in `types.rs`.
Follow the existing shape (`run_agent`, `peek_run`, `run_tokens`).

| Endpoint | Body | Returns |
| --- | --- | --- |
| `simulate_start` | `{ agent, message }` | run id, composed system prompt, tool declarations, the first messages |
| `simulate_prompt` | `{ agent, run }` | the rendered string + `Vec<Token>` + the split (instructions / tool declarations / conversation) |
| `simulate_turn` | `{ agent, run, blocks: [{ kind: "text"|"call", text?, name?, input? }] }` | executes every call in order via `tools::execute`, appends results, returns them and the resulting `stop_reason` |
| `simulate_user` | `{ agent, run, text }` | appends a user turn |

Stopping reuses the existing `stop_run`.

**Tokens.** Do not ship a tokenizer to the browser. `crates/adi-agents/src/analytics/mod.rs`
already uses `tiktoken-rs` with ranks compiled in (`ENCODING = "o200k_base"`,
`o200k_base_singleton()`); return `{ id, text, special }` per token, which is exactly
`adi_ui::Token`. Report the encoding name alongside the count, as `analytics` already does —
a token number without its encoding is a number nobody can check.

**Open decision — which wrapper to render.** The design mockup showed OpenAI's ChatML with
tools as a TypeScript `namespace functions`, and Kimi K2's real `chat_template.jinja` (tools
as one minified JSON blob in a `tool_declare` section). Anthropic does not publish how it
renders its `tools` field. The honest option for the product is to render **what the runner
actually sends**, which already exists in code: `anthropic_round`, `openai_round` and
`gemini_round` in `backends/harness/adi_loop.rs` build the request body per provider. Prefer
that over re-deriving a template, and label the encoding and provider on screen.

---

## Phase D — the page in `adi-webapp`

Wire the `adi-ui` components to the endpoints. A page under the agents section, entered from
an agent with a "Simulate" action.

The loop the page implements, which is the loop the runner implements:

1. Show the composed prompt as the model receives it.
2. The person stacks blocks into an open turn — prose and any number of tool calls.
3. **End turn** is the only thing that closes it. With calls: they execute, results append,
   the person is called again as the model — `stop_reason: tool_use`. Without calls: the run
   yields and the person answers as themselves — `stop_reason: end_turn`.
4. Flags collected while reading come out as proposed edits to the agent's system prompt.

Existing webapp constraints still apply: components from this crate need `adi-ui-type` and
explicit heights (see the note in `crates/adi-webapp/styles/tailwind.css`).

Deploy is `scripts/build-app.sh`, then `launchctl kickstart -k` the control-panel label. Never
restart ADI DNS (`adi.hive`).

---

## Tests worth writing

- Composing a prompt returns byte-identical text for a real launch and a simulated one.
- A tool call made through the simulator and the same call made by the adi loop produce the
  same result text, including the error path for an unknown tool.
- A simulated run appears in `agents runs`, and changing the agent's backend afterwards does
  not lose it (the B4 fix).
- `Ask` under an `unattended` agent refuses in a simulated run exactly as it does in a real one.
- Ending a turn with two calls appends two results, in order, before the next turn.
