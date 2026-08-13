# Agent simulator — current state

Facts only, as of 2026-08-13. What exists, what is verified, what is left.

## What the feature is

A mode where a person takes the model's seat in a real agent run: the agent's own
environment is materialized, the prompt is shown as the model receives it, the person emits
turns and calls tools, and the tools really execute.

Decisions already made by the user:

- Tools execute for real, in the agent's own cwd, with its PATH, env and secrets.
- `Ask` posts the question to the real user, exactly as a normal run does.
- Web only (`app.adi`). No CLI.
- A simulation always starts a **fresh run**. It never continues an existing transcript.
- No "what the real model did" comparison panel. Not in scope, ever.
- Build the UI components in `adi-ui` first (playground), then reuse them in the webapp.

A clickable HTML mockup of the whole flow was produced during the design conversation and
approved. It is not in the repo; it lives as a published artifact.

## Phase A — `adi-ui` components: **done**

All uncommitted, on `main`. `cargo check --target wasm32-unknown-unknown --all-targets` is
clean and every panel has been rendered and driven in headless Chrome, in both themes.

| file | what |
| --- | --- |
| `src/tokens.rs` | `Token`, `TokenStream`, `PromptText` |
| `src/toolform.rs` | `Param`, `ParamKind`, `ToolForm` |
| `src/staging.rs` | `Block`, `TurnBlocks`, `Stop`, `StopLine` |
| `src/flag.rs` | `Flag`, `FlagMark`, `FlagList` |
| `src/simulator.rs` | `Simulator`, `ToolDecl` |
| `src/chat.rs` | `Invoke` extracted `pub(crate)`, so a staged call and a real one are drawn by one component |
| `playground/main.rs` | three new panels, the last of which is the whole flow wired to itself |
| `README.md` | rows for all five |
| `Cargo.toml` | `web-sys` gains `Selection` / `Range` / `DomRect` / `Node` / `Element`, for the flag affordance |

Verified in the browser, not just compiled: a token carrying `\n` shows `⏎` **and** breaks;
a token that is only a break keeps its colour; leading spaces stay attached to the following
word; `end turn` is out at zero blocks; a turn with a call leaves the run open with
`stop_reason: tool_use`; a turn without one yields and wakes the user composer; selecting a
passage offers **flag this**, and flagging it drops a quote and a note field into the list.

One bug found and fixed by looking: the tool `<select>` rendered blank, because `picked`
started empty and the form under it had already fallen back to the first tool — a control
disagreeing with the control it controls.

## Phase B — `adi-agents`: **done**

`cargo test -p adi-agents` — **286 passed, 0 failed**. `cargo check -p adi-app -p adi-cli -p
adi-webapp-api` is clean with those crates unedited, and the crate carries no
`#[allow(dead_code)]` and no warnings of its own.

- **B1** `runner/prompt.rs` — `compose(spec, stored)` is the one composer. The four helpers
  moved out of `detached.rs`; `detached` and `pty` import them, and the `harness:adi` prompt
  export is now one `compose` call. 5 tests.
- **B2** `backends/harness/tools.rs` — `ToolDeclaration` + `pub fn declarations()`, read off
  the same `TOOLS` table the providers are built from. `run` renamed `execute`.
  `Ctx::for_conversation` extracted, so the adi loop and the simulator build the tool context
  the same way — which is what makes the shell really be the conversation's.
- **B3** `runner/human.rs` — `HumanRunner`, `RunnerKind("human")`. No child: `send` composes
  the prompt, freezes it in the state slot and marks the seat taken; `stop` vacates it;
  `events` reads the log back through `adi_events::parse`, the same parser the adi loop's
  output goes through. 3 tests.
- **B4** `SessionRecord.runner` (new `runner` column + migration) and
  `runner::runner_of(record)`. Every per-run lookup in `lib.rs` now resolves from the record
  rather than from the agent's *current* backend — `transcript`, `advance_queue`, `peek_run`,
  `stop_run`, `delete_run`, `stop`, `list_runs`, `check_deliverable`, `deliver`.
- **B5** `Agents::simulate`, `simulated_prompt`, `simulated_tools`, `simulate_turn`,
  `simulate_user`, plus `SimBlock` / `SimResult` / `SimTurn` and the two `stop_reason`
  constants. `simulate_user` delegates to `reply`, so a simulated conversation queues and
  settles by the same code every other one does.

Every test the plan asked for is written and passing, including the two that pin the
feature's central claim: the prompt a simulated run shows is byte-identical to the one a real
launch composes, and a call made from the seat returns the same text the loop's would —
error path included.

Two deliberate deviations from the plan, both to avoid the duplication the plan forbids:

- **`tools::execute` stayed `pub(crate)`**; `declarations()` is the public half. Making
  `execute` public means making `Ctx` public, and then the API crate has to assemble a `Shell`,
  an `Awaits` and a `SessionStore` itself — a second copy of `run_turn`'s setup. `Agents::simulate_turn`
  is the public door instead, and it builds the context through the shared `Ctx::for_conversation`.
- **The composed prompt is frozen in the runner's state slot** rather than recomposed on read.
  Composing means assembling a spec, and assembling a spec syncs the agent's `.bin` — a write
  path, and `simulated_prompt` is polled by a page. Frozen is also the *truer* answer: it is
  what this run opened with, on the same reasoning as `RunSpec::tool_help`.

`simulate` does not weigh the run cap: it starts no engine. A simulated run does still count
toward `run_load`, because it is a live session — worth revisiting if an open simulator is
ever found blocking real work.

## Phase C — `adi-webapp-api`: **done**

Four endpoints, one state shape. Every one answers an `AgentSimState`, so a page that stacks a
block, ends a turn, or replies gets the prompt back grown by what it just did rather than asking
again and drawing something stale in between.

| endpoint | body | answers |
| --- | --- | --- |
| `POST /api/agents/simulate` | `SimulateAgent { name, message }` | `AgentSimState` |
| `POST /api/agents/simulate/prompt` | `RunRef` | `AgentSimState` |
| `POST /api/agents/simulate/turn` | `SimulateTurn { name, run_id, blocks }` | `AgentSimTurn { results, state }` |
| `POST /api/agents/simulate/reply` | `ReplyToRun` | `AgentSimState` |

Two things worth knowing about the shape:

- **The token stream covers the conversation, not just the system prompt.** The `prompt` field is
  the system prompt; `tokens` and `sections` are the whole document — instructions, where you are,
  what you know, your tools, then one section per turn. That is not decoration: what a turn
  *appended* — the calls and above all their results — is the next thing the model reads, and a
  prompt view that stopped at the system prompt would show a reader everything except the part
  their last turn changed.
- **Tool schemas are decoded server-side** into `AgentSimField`s that map 1:1 onto `adi_ui::Param`,
  so there is no second JSON-Schema decoder in wasm. Fields come back **required first, in the
  order the tool requires them**, then the rest: a JSON object decodes into a sorted map, so
  reading `properties` back gives `background, command, timeout_ms` for `Bash` — a flag nobody sets
  above the command the call is about. The `required` array survives, and it is the tool's own
  statement of what matters.

New in `adi-agents` for this: `analytics::split` (the same encoder `analyze` counts with, so a
prompt shown and a prompt counted cannot disagree) and `runner::prompt::sections` (the cuts come
from the file that put them in — joined back together the sections are the prompt again, and a
heading an agent merely *writes about* is not a cut). 3 tests on the split.

## Phase D — `adi-webapp`: **done**

`pages/agents/simulate.rs` plus a `Simulate` state struct, four `fetch` functions, and a
**Simulate** item in an agent row's kebab. It is in the kebab rather than inline because it is not
a way of *running* the agent — it is a way of reading what the agent is told.

The page fetches and holds; everything on screen is `adi_ui::Simulator`. The one thing it assembles
is a call's arguments, which is deliberately the caller's half of the `ToolForm` bargain.

There is a **Refresh** beside Close, and it earns its place: nothing about a simulated run changes
on its own, except that `Ask` posts its question to the real user and the answer lands in the
transcript from outside the page.

## Verified against the live app

Built, deployed to `app.adi`, and driven end to end — both through the API and by clicking the real
UI in headless Chrome over CDP.

- A run of `adi-agent` shows its real **7751-token** prompt, split into `instructions` /
  `where you are`, with the section ranges exact (`concat(tokens) == prompt`).
- A `Bash` call really ran, in the agent's own cwd, and its output came back as the model would
  read it. An unknown tool answered `no tool named Frobnicate — the tools you have are: …`, which
  is the tool's own sentence, not an HTTP error.
- `stop_reason` walks `'' → tool_use → end_turn → ''` across fresh / call / words / reply.
- An ended run is listed by `agents runs` with `terminal_reason: "simulated"`, its answer head and
  `num_turns` — so `adi.agents.run.finished` publishes, with no simulator-specific code doing it.

Four bugs the live pass found and fixed, none of which a compiler would have: fields came back
alphabetical; `stop_reason` read `tool_use` on a run where nothing had been emitted; it then kept
reading `tool_use` after a person replied; and the prompt view did not show the results a call had
appended.

Every test run created during that pass has been deleted from the store; the user's own
conversations were not touched.

## Still open

The plan's one open decision — **which provider wrapper to render** — is unchanged, and the current
answer is the honest one: nothing invents a chat template. `AgentToken::special` exists and is
always `false`, because what this crate composes is a *prompt* and the wrapper around it is added
by each provider's API from the JSON body `adi_loop` sends. There is no point in the pipeline where
a rendered template exists to be split, so the seams are drawn from `sections` instead. If a
provider-accurate view is wanted later, `anthropic_round` / `openai_round` / `gemini_round` in
`backends/harness/adi_loop.rs` are what actually goes on the wire.

Two smaller ones:

- A simulated run counts toward `run_load` (it is a live session) but is not weighed against the
  cap when it starts (it launches no engine). Worth revisiting if an open simulator is ever found
  blocking real work.
- Flags are collected but go nowhere yet. The plan's step 4 — "flags come out as proposed edits to
  the agent's system prompt" — is the natural next piece.

## Environment notes that cost time

The build stall described in the previous version of this document is **gone**. It was the
`syspolicyd` wedge; after the reboot, `cargo check` for `wasm32-unknown-unknown` completes in
seconds and `trunk serve` rebuilds in about two minutes. The rest still holds:

- A process started with `&` or `nohup` from a tool shell lands in the macOS background QoS
  band at **nice 5**. A direct foreground child runs at nice 0.
- `trunk serve` binds its port only after a build attempt finishes, and on a **failed** build
  it still serves the previous `dist/`. A page that renders without the new panels is not
  evidence about the new code — check `dist/index.html`'s mtime against the file you edited.
- To screenshot: copy `dist/index.html`, inject `<style>:root{color-scheme:dark}</style>` and
  a script that deletes every panel but the one you want, then point headless Chrome at the
  copy. `data-theme` does nothing — the mount `Effect` removes it. Delete the copies after;
  trunk wipes `dist/` on rebuild anyway.
