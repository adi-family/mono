# Sessions — how a chat gets from storage into the rail

A map of every layer a session passes through on its way to a list, with the file and line
that owns each step. Written to be read before refactoring anything in that path: the point
is not *what* a session is (`docs/agent-runner.md` covers that) but **who reads it, who
reshapes it, and where the same decision is made twice**.

## Vocabulary — one thing, three names

The same object is called three different things depending on which layer you are standing on.
This is the single biggest source of confusion in this code path:

| Layer | Name | Type |
|---|---|---|
| store (`adi-agents/src/store`) | **session** | `SessionRecord` |
| agent layer (`adi-agents/src/lib.rs`, `run.rs`) | **run** | `RunInfo` |
| HTTP + client | **run** / **conversation** / **chat** | `AgentRunInfo` |
| UI rail | **session** | `SessionRow` |

`run_id` and session id are the same string. A *conversation* is a session whose backend is
answerable (a harness engine); a one-shot `process:*` run is the same record with no reply box.
A pty agent has **no session record at all** — its live pane is the whole of it, and it is
synthesized as a row only in the client.

## The pipeline is not always about one machine

Since `docs/fleet.md` §13 the rail can merge in **any number of paired nodes** at once, alongside
this machine, and then every step below runs once per selected source: each has its own
`sessions.db`, its own `Agents`, its own `runs_response`. Only the last two rows of the diagram are
local — the client's signals and the rail it draws, which is what does the merging.

What moves is the *address*, and (unlike before this was multi-select) it moves per call rather than
through one implicit pointer: `fetch::routed_for(node, path)` is a pure function from a source and a
path to the address that reaches it, called explicitly wherever a read, a mutation or a live
subscription is *about* a specific source, and `adi-app`'s `viewer::proxy` forwards each one on the
credential this machine holds for that node. So nothing in this document's pipeline is duplicated or
conditional; a session is read, listed, replied to and stopped by exactly the code below, on whichever
machine actually holds it — the client just runs the whole pipeline once per selected source and
concatenates the answers (`session_rows` → `source_rows`, `actions.rs`) the same way it already
merged across agents on one machine. The consequences worth carrying into any refactor here:

- **A `run_id` is only unique on the machine that minted it.** With several sources live at once
  this is no longer solved by clearing the rail on a switch (there is no "the" source to switch away
  from) — every row and every action on it is keyed by **`(source, run_id)`** instead:
  `SessionRow::node` for a row in the rail, `AgentsWatch::node` for whichever conversation is open.
  Deselecting a source drops its rows and closes the open conversation if it was the one on that
  source; nothing else about the rail is disturbed.
- **A node's read is polled at three seconds, not one** (`adi-app`'s `live::watchable`), because
  each tick is a mesh round trip — per selected node, all of them ticking independently; this
  machine's own copy stays at one second.

## The pipeline, end to end

```
~/.adi/mono/sessions/sessions.db            one row per session
        │  one indexed range scan per agent
        ▼
SessionStore::list(agent) -> Vec<SessionRecord>          store/mod.rs:186
        │  + runner.is_alive() per record, + advance_queue() side effect
        ▼
Agents::runs(agent) -> Vec<RunInfo>                      lib.rs:851
        │  + capability profile (interactive / answerable / caps)
        ▼
runs_response() -> AgentRuns                             handlers/agents.rs:468
        │                          ╲
POST /api/agents/runs   (one agent)  GET /api/agents/runs/all[?limit=N][&hidden=…]  (every agent)
        │  whole, hidden included    ╲   newest N + every live/asking one, `hidden`-narrowed
        ▼                              ▼
   watch.runs: Vec<AgentRunInfo>     state.all_chats / state.hidden_chats: AllAgentRuns
        │  paged() cuts it to           │  already cut and narrowed; `total` says what
        │  source_limit()               │  was left behind by the cut alone
        ╰──────────────╮   ╭───────────╯
                       ▼   ▼
        chat_all_sessions() -> Vec<SessionRow>           actions.rs:4434
        │  filter ★, sort by last_touch, partition running — `hidden` is the server's decision
        │  above; only `watch.runs`'s own un-narrowed copy is still mirrored client-side
        ▼
   adi_ui::RailGroup / SessionItem                       adi-ui/src/rail.rs, session.rs
```

---

## Layer 0 — where it is kept

Root: `~/.adi/mono/sessions/` (`Config::open()` → `adi-config/src/lib.rs:112`; the module name
is `SESSIONS_MODULE = "sessions"`, `adi-agents/src/lib.rs:79`). Override the root with `$ADI_DIR`.

```
<sessions_dir>/sessions.db              sessions, turns, queue, attachments
<sessions_dir>/settings.toml            the run cap (RunLimits)
<sessions_dir>/attachments/<id>.<ext>   one attachment's bytes, exactly as uploaded
<sessions_dir>/<agent>/<id>.log         the raw output a runner spools into
<sessions_dir>/<agent>/<id>.review.md   the dossier Analyze writes for a reviewing agent
<sessions_dir>/<agent>/<id>.<whatever>  sidecars a runner invents
```

The review dossier is a file for the same reason the log is: it is written to be **read by another
agent**, with a `Read` tool, at a path that outlives the request that made it. See
`adi_agents::review` — and note that it is deleted with the session it describes, because it is a
description of that session and nothing else.

**The log is a file and has to be**: a spawned child needs a real file descriptor to redirect
stdout and stderr into, which is not a thing a row can be. Everything else is a row, because
everything else is *listed* — see [db.rs](../crates/adi-agents/src/store/db.rs) for the profile
that settled it.

Five tables. The first four are keyed `(agent, id)`, and `turns`, `queue` and `goals` cascade off
`sessions` (`goals` is its own subject — see [goals.md](goals.md)):

| table | holds | ordered by |
|---|---|---|
| `sessions` | the record: backend, cwd, message, `started_at`, `last_activity`, `hidden`, `starred`, `runner_state`, `outcome`, `tool_help` | index `sessions_newest (agent, started_at DESC, id DESC)` |
| `turns` | one row per turn; the whole `Turn` as JSON plus `at` and `role` | `seq` |
| `queue` | what is waiting to be said next, and the images waiting with it | `seq` |
| `goals` | what the conversation is *for*: text, state, who set it, how often it has been asked | `created_at` (partial index on the open ones) |
| `attachments` | one row per uploaded image **or file**: name, media type, size, and whose message it ended up on | — |

### `attachments` is the one table that cascades off nothing

It cannot. An attachment is uploaded from a composer **before** the message that carries it exists —
often before the conversation does — so a new row has no session to point at, and a foreign key
would have to invent one. Its life instead runs:

1. **stored, unclaimed** — `POST /api/agents/attachment` writes the bytes to
   `<sessions_dir>/attachments/<id>.<ext>` and a row with empty `agent`/`session`. The id is minted by
   SQLite (`hex(randomblob(12))`), so two processes uploading in the same instant cannot agree on
   one.
2. **claimed** — recording the turn that carries it stamps the row with that conversation, *in the
   same transaction as the turn* (`transcript::insert`). Until then it is nobody's.
3. **swept, or deleted with its conversation** — an upload nobody ever sent goes after 24 hours
   (`attachments::sweep_unclaimed`, run on the way into the next upload, which is the only thing
   that creates orphans); a claimed one goes when the session does (`SessionStore::delete`).

A turn carries **references**, never bytes: `Turn::images` is `Vec<Attachment>` (id, name, media
type, size), and the page fetches each once from `GET /api/agents/attachment/<id>`, which answers
`immutable, max-age=1y` because the id *is* the version. A transcript is polled once a second, and
one that inlined its screenshots would re-send every one of them on every tick.

### An image is shown; anything else is a path

**Any type may be attached**, and the store's only refusals are an empty body and one over the cap
its kind carries — 5 MiB for an image (`MAX_BYTES`, the strictest provider's limit), 25 MiB for
anything else (`MAX_FILE_BYTES`, kept under `adi-app`'s 32 MiB body cap so the *handler* refuses an
oversized upload and can name the file in the sentence). The **four image types** are the ones a
provider will take in a request body; a PDF, a CSV or a log is stored exactly the same way and never
goes into a body at all. Its bytes being on the machine the run happens on is the point — the
message tells the agent where they are, and the agent opens them with its own tools.

The stored file is named `<id>.<ext>`: an image by its media type (a pasted screenshot has no
filename to go by), everything else by the extension it arrived with, sanitised to lowercase ASCII
alphanumerics and `bin` when there is nothing usable. The extension is load-bearing — a file-reading
tool decides what it is looking at by the name.

### Two ways an image reaches a model

`Runner::image_delivery` answers *how*, and the difference is where the picture ends up rather than
whether the model sees it:

| delivery | engines | what happens |
|---|---|---|
| `Inline` | `harness:adi` | the bytes go into the request body this tree writes, as base64 in each provider's own shape — and the message also names the file, so a picture that is the *material* (crop it, resize it) can be reached and not only seen |
| `Path` | `harness:claude-sdk`, `process:claude`, `process:codex` | the message gets a block naming the files, and the engine opens them with its own file-reading tool |
| `None` | `pty:*`, a simulated run | nothing to send to — a pane is typed into, and a simulation has a person in the model's seat |

A **file** that is not an image gets its block whatever the delivery, because no provider here takes
one in a body: `Inline` and `Path` both name it, and only a `None` engine — with no tool to open it
— is told nothing.

Both blocks are rendered by `with_attachment_paths` (`lib.rs`), and reach an engine by one of two
routes. A path-delivery engine is *handed* its message, so `for_engine` prepares it on the way out.
The adi loop is handed an agent and a conversation instead and rebuilds its messages from the store,
so it calls the same renderer while seeding (`words_of` in `adi_loop.rs`) — exactly as it rebuilds
the pre-run block. Either way the paths are deliberately **not** what the transcript records: they
are directions for one engine, so a reader would see plumbing under a thumbnail it is already
drawing, and a turn replayed later to a different engine would carry instructions that mean nothing
to it.

A message with an attachment to a `None` engine is **refused** (400) rather than recorded looking
sent.

The id is `{unix_millis:013}-{seq:04}` (`store/record.rs`), so it carries its own start time and
sorts by it.

Two of those columns are write-once, and both are written under an `IS NULL` gate so that whoever
gets there first decides and everyone after is a no-op:

- **`outcome`** — how the run ended (`RunOutcome`: the engine's `terminal_reason`, `is_error`, cost,
  duration, the head of the answer). There is no reaper, so an ending is noticed by whoever lists
  the run first; the gate is what makes the accompanying `adi.agents.run.finished` fire exactly once
  across the app, the CLI and every trigger's child (`Agents::note_finished`, `lib.rs`).
- **`tool_help`** — the rendered tool section this conversation opened with, re-used by every later
  turn instead of being derived again (`pin_tool_help`, `lib.rs`). Deriving it per turn made the
  same conversation's system prompt differ between turns — each tool is asked to describe itself
  under a shared time budget — which invalidated the whole prompt cache behind it.

Both were added to a table that already existed on every machine, so they arrive through
`db::MIGRATIONS` (`ALTER TABLE … ADD COLUMN`, error swallowed) rather than through `SCHEMA`, which
`CREATE TABLE IF NOT EXISTS` makes a no-op against an existing store. So did `starred`, and the case
is pinned by `a_store_written_before_the_column_reads_back_with_it` (`store/mod.rs`) — a column added
to `SCHEMA` alone reaches new stores and nowhere else, and the first `SELECT` naming it then fails
*every listing at once* on exactly the machines with history to lose.

### The two flags on a row are not the same kind of thing

`hidden` and `starred` look like a pair and are not:

- **`hidden`** is purely a listing preference. The store returns hidden sessions like any other —
  filtering is a *reader's* job, not the store's — and nothing else in the system reads it.
  `GET /api/agents/runs/all`'s `?hidden=` is where that reader now lives for the rail (Layer 3):
  the client no longer re-implements the rule, it asks for the view it wants.
- **`starred`** is a person saying *keep this*. It is read by two things that are not views:
  `prune_old` skips a starred session however old it is (`store/mod.rs`), and the HTTP paging cut
  lets one ride free past the limit (`newest`, `handlers/agents.rs`). A star that the cap swept
  anyway, days later, would be the one outcome the mark exists to prevent.

Neither clears the other: a conversation can be put away and kept at once.

WAL, `busy_timeout = 5000`, `synchronous = NORMAL` — the CLI, the app, and every trigger's child
open this independently, and the pragma order is load-bearing (`busy_timeout` first, or switching
journal mode fails against a store another process is mid-write on).

**Connections are thread-local** (`store/db.rs`). Not an optimization: `Agents::sessions()` builds a
store once per agent *and* again per idle run, so one listing constructs it several hundred times.

## Layer 1 — `SessionStore`: rows → records

`crates/adi-agents/src/store/`, entry point `SessionStore::list` (`store/mod.rs:186`).

One statement:

```sql
SELECT ... FROM sessions WHERE agent = ?1 ORDER BY started_at DESC, id DESC
```

By start, not by activity: the rail's own ordering is a view's business, and a listing that
reshuffled itself as answers landed would be a different thing to page through. That pair is the
index, so it is a range scan rather than a sort.

Two invariants worth knowing before touching this:

- **`last_activity` is a column, written only by `append_turn`** (`store/transcript.rs`), in the
  same transaction as the turn. That is the whole of its meaning: a listing sorted by when a
  conversation last *spoke* must not move because the chat was read, spooled into, or hidden. It was
  once derived per read from file mtimes, and every one of those things moved it.
- **The row is what says a session exists.** A stray `<id>.log` with no row is not a session. The
  file store answered otherwise because it wrote sidecar and log separately and a crash between them
  orphaned real output; here the row is committed before a runner is ever handed the log path.

The store answers **the full history, including hidden sessions**. Hiding is a column, never a
filter — filtering is the view's job.

## Layer 2 — `Agents`: records → runs, plus liveness

`Agents::runs` (`lib.rs:851`) is the only public listing verb.

```rust
runner_for(&agent.manifest.backend)          // registry.rs:17 — the one Backend → behaviour map
  ├─ None                    => vec![]       // an unknown/plugin backend lists nothing
  ├─ as_terminal().is_some() => vec![]       // pty: the live pane IS the run, no history
  └─ else => list_runs(...)                  // lib.rs:879
```

`list_runs` maps each `SessionRecord` to a `RunInfo`, asking the runner `is_alive(session)` per
record (`runner/detached.rs:252` — reads the pid out of the record's `runner_state` slot and
verifies its start time, so a recycled pid never reads as alive).

**`runs()` has a side effect.** After listing, it advances the queue of every run it just saw as
idle (`lib.rs:861-870` → `advance_queue`, `lib.rs:622`), and re-lists if anything started. So
*listing a chat is what keeps queues moving* while you are reading some other chat. Any refactor
that makes the listing "pure" silently stalls every queue.

**A running turn is the other thing that drains a queue.** Only the reader above can start a *new*
turn, but a `harness:adi` turn takes what is waiting between its own rounds
(`backends/harness/adi_loop.rs`, `take_queued` → `SessionStore::take_queued_as_turn`), so a message
typed mid-answer reaches the model within one round instead of waiting for the answer. It is a
different verb from `dequeue` because it also records the message as a user turn, in the same
transaction: there is no launch behind it to write it down. Two consequences worth knowing — a
conversation's `turns` really can hold two `user` rows in a row, and something the transcript replay
in `Wire::seed` has to merge; and the parent must never take from a queue whose turn is alive
(`advance_queue` checks `is_alive` first), or the same conversation is answered twice at once.

`Agents::sessions()` (`lib.rs:295`) only constructs the store — a `PathBuf` and nothing else. Keep
it that way: it is built once per agent *and* again per idle run inside `advance_queue`, so
anything with a cost in it is paid a few hundred times per `/api/agents/runs/all`.

Ordering note: `RunInfo` comes out sorted by `started_at` (from the store) and *is not re-sorted*
here.

## Layer 3 — HTTP

Handlers: `crates/adi-webapp-api/src/handlers/agents.rs`. Routing: `crates/adi-app/src/main.rs:609-621`.

| Route | Handler | Answer |
|---|---|---|
| `POST /api/agents/runs` | `agent_runs` :190 | `AgentRuns` — one agent's history, whole |
| `GET /api/agents/runs/all[?limit=N][&hidden=false\|true]` | `all_agent_runs` :457 | `AllAgentRuns` — every agent, one round-trip; `?limit` pages it, `?hidden` narrows it |
| `POST /api/agents/run/peek` | `peek_run` :205 | one run's transcript/log snapshot — whole, or a folded page (below) |
| `POST /api/agents/run/steps` | `run_steps` | the calls behind one folded run |
| `POST /api/agents/run/hide` | `hide_run` :439 | flips `hidden`, replies with fresh history |
| `POST /api/agents/run/star` | `star_run` | flips `starred`, replies with fresh history |
| `POST /api/agents/run/delete` | `delete_run` :419 | deletes, replies with fresh history |
| `POST /api/agents/run/stop` | `stop_run` :400 | stops, replies with fresh history |
| `POST /api/agents/run/reply` | `reply_run` :340 | sends/queues a message, replies with a snapshot |

Every one of those goes through `runs_response` (`agents.rs:468`), which is where the per-backend
capability profile is attached (`agent_caps` → `adi_agents::capabilities`) and where each run's
`message` is cut to a 300-character title (`title_of`). The whole message is never lost — it is the
conversation's first turn — and sending all of it made this answer 1.4 MB to fill a rail that shows
72 characters of each.
`interactive` and `answerable` on the wire are what decide **chat vs. log** in the client.

DTOs live in `crates/adi-webapp-api/src/types.rs:1049` (`AgentRunInfo`), `:1073` (`AgentRuns`),
`:1093` (`AllAgentRuns`). Note `#[serde(default)]` on `last_activity` — an older server omits it
and the client must fall back to `started_at`.

`GET /api/agents/runs/all` is O(agents × sessions) directory walks per call. It is in
`SHARED_GETS` (`adi-app/src/main.rs:434`), so concurrent identical requests are collapsed into
one computation — keyed on the **full path**, query included, so two pages of it are two answers
rather than one shared by mistake.

### Narrowing (`?hidden=`)

The decision that used to be the client's — `runs.filter(pending_question.is_some() || !hidden)` —
moved here, so the rail no longer re-implements a rule the server can just answer.

- **`?hidden=false`** is the rail's main listing: everything **not** hidden, plus a hidden run that
  is asking a question nobody has answered (`filter_by_hidden`, `agents.rs`) — the same escape
  hatch [`newest`] and `source_rows` already carried, moved rather than duplicated. This is what
  `state.all_chats`/`state.rail_node_chats` hold on the chat home.
- **`?hidden=true`** is the mirror, for the rail's own Hidden band: hidden runs, minus one already
  asking a question, which the main listing above already shows and must not draw twice. Fetched
  into `state.hidden_chats`/`state.rail_node_hidden_chats` **only while the band is open**
  (`crate::state::refresh_hidden_chats`) — not on the rail's ordinary poll, since a band nobody has
  opened has nothing worth spending a request on.
- **Left off entirely**, the answer is whole, hidden runs included — what a workbench
  (`all_chats_flatten`) asks for, what Analytics and the Agents index still ask for, and what a
  paired node running a binary from before this parameter existed keeps sending regardless of what
  a newer client asks it for. No client-side guard papers over that gap: the same "an older
  server answers whole and the client renders it as given" rule this file already documents for
  `limit`/`before`/`fold` on `peek_run` applies here too, rather than reintroducing the filter this
  change exists to remove from the client.

### Paging (`?limit=N`)

The rail asks for a page; every other reader asks for all of it.

- **The cut** (`newest`, `agents.rs`) is the newest `N` sessions **across every agent**, by
  `max(last_activity, started_at)` — one flat list, as the rail reads it. A per-agent cut would
  spend the budget on agents nobody has touched in months. Applied *after* `?hidden=`, so it counts
  the population that narrowing actually left rather than the whole store.
- **Three kinds ride free**: a session that is *running*, *blocked on a person*, or *starred* is
  kept whatever its age and without spending the budget. The first two are the rail's other bands,
  and they are inboxes rather than history — a question asked months ago and never answered is
  exactly the row paging must not swallow. The third is the same argument made by hand.
- **Every agent is still listed**, runs or none: an interactive agent has no runs to begin with
  and still contributes a rail row, and the client reads `caps` off this same listing.
- **`total`** counts what exists **after `?hidden=`, before `?limit=`** — not what was sent. `total
  − Σruns` is what a Load more prints how many are left behind: summed over every selected source
  for the single-list layout's combined button (`chat_load_more`), or read for one source alone by
  that source's own block (`band_load_more`, `SessionGroup::Machine` with several sources ticked).
  A zero there is what removes the button. Narrowed the same way the runs themselves are, so the
  count and the rows it describes never disagree about what "exists" means.
- **Each source pages itself**, not a shared rail-wide budget divided between them:
  `state::source_limit` reads this machine's own `state.rail_limit` for `None`, or one ticked
  node's own entry in `state.rail_node_limits` — [`SESSION_PAGE`] before that source's own Load
  more has ever been pressed. Pressing the single-list layout's combined button bumps every
  selected source's own limit at once; pressing one block's own button in the per-machine layout
  bumps only that source's. The page exists because every selected source's index is watched over
  the live channel and re-sent on every move; asking every source for everything at once would page
  nothing at all. Unticking a node drops its entry from `rail_node_limits`, so re-ticking it starts
  at the first page again rather than resuming a stale wider one.

`POST /api/agents/runs` is *not* paged — the open conversation has to be findable in it — so the
client cuts the watched agent's copy itself (`paged`, `actions.rs`) by the same rule. **The three
free-riding kinds have to be listed identically in both places.** A session the server kept and the
client then cut would go missing from the rail of the agent you are on, which is the one agent whose
history arrives whole and so the only place the disagreement would ever show.

### The lazy transcript (`limit` / `before` / `fold`)

`peek_run` used to answer with the whole conversation — every turn, every call, every tool result —
and it is re-answered **once a second** for as long as somebody has a chat open (Layer 4). Measured
on this machine's store: 141 MB of turn JSON over 5,423 turns, of which **84% is tool input and
output**; the longest single conversation is 3.4 MB across 46 turns and 1,226 calls.

`TranscriptView` (flattened into `RunRef`, `ReplyToRun`, `AnswerRun`, `UnqueueFromRun`) is the opt-in
that makes it a page:

- **`limit`** — the newest N turns. Each turn carries its own `seq`, because a page cannot be
  counted: turn 83 arrives third in a window of twenty, and every anchor, rail link and step request
  is built from the 83. `total_turns` says what the page is a window onto.
- **`before`** — the N turns below this one, for "earlier messages". Cut in SQL
  (`transcript::load_page`), so older turns are never decoded. Carries no answer-in-flight and no
  queued messages: those live at the end of the conversation.
- **`fold`** — each run of tool calls (a maximal stretch of calls and thinking, broken by anything
  the agent *said*) becomes one `AgentStep::Calls` header: count, tool names, the head call's
  preview, its status. That is **exactly** what a closed receipt draws (`adi_ui::Did`), so a folded
  transcript and an unfolded one are the same screen until somebody opens a run — at which point
  `POST /api/agents/run/steps` fetches that one range. `fold` also drops the 64 KB log tail from
  `output`, which no transcript reader draws.

Every field defaults to the old behaviour, so the CLI, an older panel and a paired node running last
month's binary all get what they always got — and an older *server* ignores them and answers whole,
which the client still renders.

**The numbers ride with the page** (`AgentPeek::stats`, `AgentChatStats`), because they are what a
page loses: the analytics rail counting its twenty turns would report a hundred-turn conversation as
twenty. They are counted **incrementally** — a transcript is append-only, so what turns 0..N add up
to cannot change (`recorded_stats`, keyed by conversation and turn count) and each turn is counted
once rather than once a second.

Measured on the 3.4 MB conversation, end to end through the endpoint:

| Request | Bytes | Time |
|---|---|---|
| whole, unfolded (what every other caller still gets) | 3,520,191 | ~520ms |
| `limit: 20, fold: true` (what the chat asks) | 42,873 | ~48ms |
| `POST /run/steps` for one opened run | 3,323 | ~14ms |

Part of that speed-up is not the paging at all: `settle` — the lazy clock that commits a finished
answer before every read — was loading the entire transcript to look at its **last** turn, ~180ms
per read on that conversation. It reads one row now (`SessionStore::last_turn`).

## Layer 4 — transport

Two paths, same shape:

- **Live channel (default).** `/api/ws` re-dispatches watched reads server-side on a timer and
  pushes only when the answer changed. The allowlist *is* the security boundary
  (`adi-app/src/live.rs:74`): `/api/agents/runs/all` is `SLOW` (3s), `/api/agents/runs` and
  `/api/agents/run/peek` are `FAST` (1s). The allowlist matches the **route** (query stripped);
  the *topic* is keyed by the full path, so each page of the index is its own topic.
- **Polling fallback**, when the socket is down: `adi-webapp/src/main.rs:203-250` (`refresh`),
  a 4s tick calling `fetch::all_agent_runs_visible(limit)` (`fetch.rs`) for this machine's own page.

Pressing a source's own **Load more** widens its own limit (`state.rail_limit` for this machine,
that node's entry in `state.rail_node_limits` for a paired one — `state::source_limit` reads
whichever) — read *tracked* where the subscriptions are built, so the effect re-runs, that source
re-subscribes at its wider path, and the next page arrives at once rather than at the socket's next
tick. The single-list layout's combined button widens every selected source's limit in one press;
a per-machine block's own button widens only its own. Ticking a source on or off adds or drops its
own subscription without touching any other source's limit. **Earlier messages** widens the transcript the same way
(`watch.turn_limit`, read tracked in `chat_subscriptions`) — and because a topic is keyed by
`method path\nbody`, a wider page is simply a different topic.

Client subscriptions are declared in `state::chat_subscriptions` (`state.rs:2024`) and
`state::subscriptions` (`state.rs:1836`) and installed via `live::watch`
(`adi-webapp/src/live.rs:180`). Every write uses `set_if_changed`, so an unchanged list never
re-renders.

## Layer 5 — client state

`crates/adi-webapp/src/state.rs`. **Two signals hold this machine's session lists, and both feed
the rail:**

| Signal | Source | Scope | Declared |
|---|---|---|---|
| `state.all_chats: Option<AllAgentRuns>` | `GET /api/agents/runs/all?hidden=false` | every agent, this machine, main-listing only | `state.rs:67` |
| `watch.runs: Vec<AgentRunInfo>` | `POST /api/agents/runs` | the agent on screen, its own source, whole (hidden included) | `state.rs:1289` |

They overlap on purpose: `watch.runs` is refreshed faster and is updated *synchronously* by
mutations (hide/delete reply with fresh history), so the row of a chat you just deleted leaves
immediately instead of at the next 3s tick. `chat_all_sessions` (by way of `rail_bands` →
`session_rows` → `source_rows`) prefers `watch.runs` for the watched agent *when the watched
conversation is on that same source*, and falls back to the source's own index otherwise. Because
`watch.runs` is never `?hidden`-narrowed, `source_rows` still mirrors that one exclusion
client-side for rows drawn from it — see Layer 3's "Narrowing".

A third signal, `state.hidden_chats: Option<AllAgentRuns>` (`GET …?hidden=true`), backs the rail's
Hidden band and nothing else. Unlike the two above it is not part of the chat home's ordinary
subscription set — it exists in that set only while [`State::show_hidden`] is true, so opening the
band adds it and closing the band drops it, rather than paying for an answer nobody is reading.

That preference is why paging is done twice. `all_chats` arrives already cut; `watch.runs` is the
watched agent's *whole* history, so `paged` cuts it client-side — otherwise the agent you are
actually on would be the one agent paging did nothing for, which on this machine is the one with
nearly every session.

**Multi-select (`docs/fleet.md` §13) adds one source's worth of signals per paired node ticked in
the rail's node menu**, kept apart from the pair above rather than folded into them:

| Signal | Source | Scope | Declared |
|---|---|---|---|
| `state.rail_node_agents: BTreeMap<String, AgentsState>` | one node's `GET /api/agents` | that node's agents, keyed by petname | `state.rs` |
| `state.rail_node_chats: BTreeMap<String, AllAgentRuns>` | one node's `GET /api/agents/runs/all?hidden=false` | that node's main listing, keyed by petname | `state.rs` |
| `state.rail_node_hidden_chats: BTreeMap<String, AllAgentRuns>` | one node's `GET …?hidden=true` | that node's Hidden band, fetched only while it's open | `state.rs` |

This machine is never a key in either map — `state.agents`/`state.all_chats` already hold it, kept
fresh by the poll and the live channel that existed before multi-select did, and a second fetch of
the same answer under a `None` key would be two clocks telling the same fact. `session_rows` reads
`state.session_local`/`state.session_nodes` to decide which sources are live, then runs the same
per-agent merge (`source_rows`) once against each one's pair of signals and concatenates the rows
before the rest of the pipeline (filter, sort, band) proceeds unchanged. `AgentsWatch::node` and
`SessionRow::node` are what key a row and an action on it to the right source — see §13's
"Multi-select" for the full reasoning.

Other state that shapes the list: `state.rail_limit` / `state.rail_node_limits` (each source's own
page, `SESSION_PAGE` = 100 to start — see `source_limit`), `state.rail_collapsed_bands` (which
per-machine blocks are folded shut), `state.starred_only`, `state.show_hidden`, `state.session_menu`,
`state.chat_drawer`.

**`starred_only` is about agents, not conversations**, and is the one genuine naming collision in
this path. It narrows the rail to the sessions of agents starred on the Agents page (a field on the
*manifest*); a conversation's own star is a column on its session row. Same glyph, two marks.

Both row flags are written through one path: `set_session_hidden` / `set_session_starred` post, then
hand the answer to `settle_session_flag`, which sets `watch.runs` for the watched agent and re-fetches
the index **at the rail's current page** — an unlimited refetch would widen the rail to the whole
index until the socket's next answer narrowed it back.

## Layer 6 — render

`crates/adi-webapp/src/pages/agents/actions.rs`, from `chat_home_view` (`:2571`).

```
chat_rail                               the whole left rail
├─ chat_all_sessions                    one list, or one block per machine
│   ├─ rail_bands                       the bands as drawn; also what ⌘1…⌘9 and ⌘⌫ read
│   │   ├─ session_rows                 the rows, before anything bands them
│   │   │   ├─ source_rows  × selected source  the merge (`docs/fleet.md` §13) — one machine's own
│   │   │   │   │                              agents plus one call per selected node, concatenated;
│   │   │   │   │                              each source's own list already came server-narrowed
│   │   │   │   │                              to `?hidden=false` (Layer 3) — hidden stays hidden
│   │   │   │   ├─ ★ filter, per source        each source's own starred agents, never another's —
│   │   │   │   │                              a run with pending_question is kept regardless
│   │   │   │   ├─ per agent: pty ⇒ one synthetic row (when: now); else that agent's runs, plus —
│   │   │   │   │             for the *watched* agent's own un-narrowed `POST /api/agents/runs`
│   │   │   │   │             copy only — runs.filter(pending_question.is_some() || !hidden)
│   │   │   │   └─ paged                       the *watched* agent's own list, cut to its own
│   │   │   │                                  source's page (`source_limit`)
│   │   │   ├─ "Mine" filter            launched_by == human — a run with pending_question is
│   │   │   │                            kept regardless of who launched it
│   │   │   └─ sort by last_touch desc  last_touch = max(last_activity, started_at)
│   │   ├─ activity_bands               five partitions: asking, running, awaiting, starred, the
│   │   │                               rest — the rail's reading ORDER, not headings any more;
│   │   │                               every filter above carries a pending_question escape hatch,
│   │   │                               so an asking run always reaches this partition to be found
│   │   ├─ SessionGroup::Flat ⇒         concatenate the five back into one unlabelled band
│   │   ├─ SessionGroup::Machine ⇒      re-deal those five into one band per source, this machine
│   │   │             (the default)     first (BTreeMap on Option<node>), activity order kept
│   │   │                               inside; one source ⇒ one band, and its label is dropped
│   │   ├─ drop empty bands, number the first 9 rows of every band *not folded shut*
│   │   │                               (`state.rail_collapsed_bands`) — no cap: a band draws every
│   │   │                               row it holds, open or not
│   │   └─ For(keyed "node:agent:run_id") -> chat_session_row
│   ├─ one band ⇒                       a single list filling the rail, `chat_load_more` under it —
│   │                                   asks every selected source for its next page at once
│   └─ several bands ⇒                  one `chat_machine_block` per source, sharing the column's
│                                       height equally (`flex-1`) and each scrolling on its own;
│                                       folding one (`rail_collapsed_bands`) gives its share back to
│                                       the rest; each carries its own `band_load_more`, for its own
│                                       source's page only
└─ chat_hidden_sessions                 the collapsed Hidden band, fetched only while open
                                         (`state.hidden_chats`/`rail_node_hidden_chats`,
                                         `?hidden=true`, Layer 3) — already excludes a run with
                                         pending_question, which the main listing above shows instead
```

`chat_session_row` maps a row to `adi_ui::SessionItem` (`crates/adi-ui/src/session.rs`) inside a
`RailCard` (`crates/adi-ui/src/rail.rs:38`). `SessionState` has five states but the rail only
ever produces four: `Waiting` when the conversation is asking, `Working` when it is running,
`Awaiting` when it has stopped holding a registered [wake](../crates/adi-agents/src/awaits.rs),
else `Done` — `Error` is unreachable from this path today. The three live states are tried in that
order, so a run that is working *and* holding a wake for what it launched is a working run.

**The state is on the row, not in a heading over it.** A 6px dot and one word in the meta line,
between the agent's name and the age: amber + "your answer" when it is stopped on a person, the
rail's one orange dot + "working" while a turn is in flight, a grey dot + "coming back" when it is
holding a wake — and nothing at all when it is done, which is most of the rail and the reason a
marked row is worth looking at. The mark travels with the row into whatever band the rail is drawn
in, which is what lets the bands be machines (below); a heading can only answer one question, and
"where is this running" is the one a merged rail cannot answer any other way.

**The activity order is deliberate and the starred partition comes fourth.** Waiting, running and
awaiting are states a conversation is in *now* and will leave on its own; a star is a standing
instruction. A starred chat that happens to be working is still sorted with what is working, so the
partition collects only the ones recency ordering would otherwise have carried off — which is the
whole reason to mark one.

**There is no cap any more.** `chat_all_sessions` draws exactly one band's worth of layout: a
single band (one selected source, or `SessionGroup::Flat` merging several) is a plain list filling
the rail, with one combined **Load more** (`chat_load_more`) under it; more than one band
(`SessionGroup::Machine`, several sources ticked) is one `chat_machine_block` per source, each a
flex item sharing the column's height *equally* (`flex-1`) and scrolling on its own
(`overflow-y-auto`) — collapsing one (its own header, click to fold — `state.rail_collapsed_bands`,
by label, page state) gives its share back to the blocks still open, down to the last one taking
the whole column. A band, folded or not, still draws every row it holds; folding only changes
whether that list is on screen, never what is in `RailBand::rows` — `chat_inbox` and the Hidden
band's own bookkeeping read every row of every band regardless, because a question left behind a
folded block nobody opened is a run stopped for good ([`drawn_rows`] is the one place collapse is
applied, and only for the reading order ⌘1…⌘9 and ⌘⌫ walk).

Two "Load more"s, deliberately different depending on the layout: the single-list layout has one,
asking every selected source for its next page at once (bumping each of their limits — this
machine's `state.rail_limit`, one entry per ticked node in `state.rail_node_limits`); the
per-machine layout gives every block its own (`band_load_more`), asking only that block's own
source for its own next page. Neither reveals rows already in hand — unlike the old "Show N more"
this replaced, everything a band holds is already on screen, so both always ask the backend.

⌘1…⌘9 number the first nine rows of every band **not folded shut**, straight down the rail and
across the blocks — a row inside a collapsed block carries no number, since nothing draws it to
open. ⌘⌫ rides alongside them (`install_session_hotkeys`, `actions.rs`): it hides — or, struck again
on an already-hidden one, unhides — the conversation open in the centre pane, exactly what the
row's right-click menu's Hide/Unhide does. Reading whether it is currently hidden is *not* read off
`rail_bands`: since a hidden run now leaves the main listing server-side (Layer 3), a conversation
opened hidden from the Hidden band has no row there at all, and the lookup would silently read
"not hidden" every time — instead it reads `watch.runs`, the watched agent's own un-narrowed copy
(`POST /api/agents/runs`), where the open run is always findable by its `run_id`. It declines
untouched when the target is a text field (⌘⌫ there means "delete to line start"), when nothing is
open, or when the open pane is a pty agent's live session, which has no run behind it to hide. On
the hide direction only — unhiding moves nothing — it also hands the pane on: the row that comes
after the one just hidden, in the same order ⌘1…⌘9 walk minus whatever is folded shut
([`drawn_rows`]), wrapping from the last drawn row back to the first, so striking the key over and
over walks the rail from the top putting each chat away in turn. A drawn list of one row (the one
being hidden, nothing else visible) has no successor, and the pane goes to empty exactly as it
always did before this. The successor is worked out from `rail_bands` *before* `set_session_hidden`
is called, then opened right after it returns rather than after `settle_session_change`'s async
rail refresh lands — `set_session_hidden` has already run `close_run_view` synchronously by then,
so opening the next row in the same call is what keeps the pane from flashing through the empty
state, and `settle_session_change` only ever overwrites `watch.runs` for the agent that was just
hidden, which is harmless whether or not that turns out to be the same agent `next` moved to.

**Two groupings, and the default is Machine** (`SessionGroup`, `state.rs`), from the **Group by**
half of the head's filter menu:

- **Machine** — one band per selected source, this machine first and then each ticked node in name
  order, with the five activity partitions deciding the order *inside* each one. **Two or more
  sources ⇒ one `chat_machine_block` per band**, sharing the rail's height equally and each
  scrolling on its own, foldable to just its header and back (`state.rail_collapsed_bands`) — see
  above. Rows then stop printing their own source on the meta line, since the block's own header
  says it (`chat_session_row`'s `sourced`); the "Waiting on you" inbox under the composer keeps
  printing it, because that list mixes machines under one heading of its own. A source with nothing
  to show draws no block at all: "this machine has nothing" is a claim, and while a newly-ticked
  node's two fetches are in flight it would be a false one. **One source is the other layout
  entirely** — a single list, no block chrome, nothing to fold, and the label the one heading would
  have carried is dropped, since it would only name the machine you are reading it on — which is
  what makes this a safe default for a machine paired with nobody: it *is* the flat list until a
  node is ticked.
- **One list** — every session in one unlabelled band, in the same activity order. What the rail
  looks like with nothing paired, available as a choice for an operator who wants a merged fleet
  read by recency rather than by machine.

There is no "Activity" grouping any more, and that is the point: a state now travels with its row
(above), so banding by it would spend the rail's one heading ladder — a 264px rail has room for one,
and a band inside a band at the same 12px reads as two bands of the same kind — on the question the
row already answers, and take away the one it cannot. Unlike the narrowing beside it the choice is
**persisted** (`adi-session-group` in `localStorage`), because it is a preference about the
selection in `session_nodes`, which is itself persisted; a browser holding the old `activity` value
reads as **One list**, the option that kept that grouping's flat reading order.

**Awaiting is not an inbox.** Its rows sort below what is running, because nothing is happening in
the conversation this second, and above everything else, because something is going to: an await is
the run's own note saying *wake me when…*, and a rail that read it as finished would say the one
thing that is false about it. Nobody has to do anything about one, which is why its row does not
breathe the way a question's does and why it says "coming back" in plain ink rather than in a
colour. The listing carries the awaits (`AgentRunInfo::awaits`), so the mark costs no second
request — and `newest()` holds an awaiting session through the page cut for the same reason it
holds a running one.

Each row carries two controls on its right edge, in separate absolute anchors rather than one flex
row (both buttons are `position: absolute` against whatever anchor they are given, so a row of them
would stack): the **star** at `right-7`, always lit once it is on, and the **delete** at `right-1`,
only under the cursor. The auto-hide for the delete lives in the row's own utility classes; the
`.adi-chome__sessionrow` rules in `main.scss` gate the *Hidden band's* copies, which build their own
markup.

The `For` key is `"{agent}:{run_id}"` and is load-bearing, not tidiness: a row's click handler is
bound when the row is *built*, so an unkeyed list rebuilt with a different shape (which is
exactly what toggling ★ does) opens whichever session used to sit in that slot.

### The second list

The Agents page and each project's Agents panel render a *different* list from the same data:
`all_chats_view` (`actions.rs:627`) → `all_chats_flatten` (`:652`) → `all_chats_rows` (`:687`),
a sortable table — and, unlike the rail, always this machine's own: it has no node menu and no
concept of a second source. It differs from the rail in three ways worth knowing before unifying
them:

- it sorts by **`started_at`**, not `last_activity` (`:681`);
- it does **not** filter `hidden` — a workbench shows everything (`:4038` explains the rule);
- it filters by *project* instead of by ★.

---

## A single session, traced

1. `Agents::launch_run` (`lib.rs:462`) resolves the agent, checks the run cap, builds the
   `RunSpec`, then `store.create(...)` mints `<millis>-<seq>` and inserts the row. `cwd` is pinned
   here, forever.
2. The opening message is appended as a user turn (`lib.rs:497`), which is also what sets
   `last_activity` to a real moment.
3. `runner.send(...)` spawns the child; the runner creates `<id>.log` and parks its pid in
   `runner_state`. The log existing is what `has_started()` means (`store/session.rs:78`).
4. `store.prune_old(...)` (`lib.rs:504`) — *after* the new files exist, so the new run is never
   counted among the old ones.
5. Next tick: the server recomputes `/api/agents/runs/all?hidden=false`, the answer differs, the
   socket pushes it, `state.all_chats` is set.
6. `chat_all_sessions` re-runs: the row already passed the server's `hidden` narrowing, its agent
   passes ★, `last_touch` is now, `running` is true → it sorts to the top of its machine's band,
   with an orange dot and "working" on it.
7. When the turn ends, `is_alive` goes false; the mark goes with it and the row sinks into the
   finished tail — or keeps a grey dot and "coming back", if the run registered a wake before it
   stopped (which launching another agent does for it). The answer is committed
   to the transcript by `settle` (`lib.rs:1131`) — which happens **when the chat is opened/read**,
   and deliberately stamps the turn with the log's mtime, not `now`, so committing an old answer
   does not shove that chat back to the top.

## Ordering — decided in five places

| Where | Key | Note |
|---|---|---|
| `sessions_newest` index | `started_at` desc | the store's contract; deliberately *not* activity |
| `lib.rs:879` | — | preserved, not re-sorted |
| `actions.rs` (rail, `session_rows`) | `last_touch` desc, stable | pty rows stamped `now` sort first |
| `actions.rs` (rail, `rail_bands`) | activity, then the above | one list, or one band per machine |
| `actions.rs:681` (table) | `started_at` desc, user-sortable | disagrees with the rail on purpose |

## Why a session might not be in the list

Work down this list when one is missing:

1. Its agent's backend has **no runner** (`Backend::Other`) → `runs()` returns `[]` (`lib.rs:852`).
2. Its agent is **pty** → no history by design; the rail synthesizes one row, and only when the
   session is live or that agent is on screen (`actions.rs:4214`, in `source_rows`).
3. `hidden: true` → left out of `?hidden=false` server-side (`filter_by_hidden`, `handlers/agents.rs`),
   so out of the main bands and, once the band is opened, in the Hidden one instead
   (`?hidden=true`, `chat_hidden_sessions`) — **unless** the run holds a `pending_question`, which
   stays in the rail proper, marked and at the top of its band, instead of going to the Hidden one:
   hiding is "out of my sight", but a question addressed to a person outranks that, because it is
   transient and leaves on its own the moment it's answered, whereas a hidden run holding one would
   be stuck for good.
4. **★ is on** and its agent is not starred on *its own source* (`source_rows`, `actions.rs`) — off
   by default, so this only applies once someone has switched it on this page load. Note this is the
   head's *agent* filter, which is a different mark from a conversation's own star, and — since
   multi-select — a fact about one source's agent list that a same-named agent on another source
   does not inherit. A run with a pending question is the one exception: it is kept whatever this
   filter says about the agent it belongs to.
5. **"Mine" is on** (the default) and the run's `launched_by` isn't `human` — the common case for a
   run a subagent launched for itself, which is otherwise the majority of what a busy fleet accrues.
   A pending question is the exception here too: a subagent-launched run stopping to ask a person is
   exactly the run an operator has to be able to see and answer, filter or no filter.
6. Its `chat_machine_block` is **folded shut** (`state.rail_collapsed_bands`, `SessionGroup::Machine`
   with several sources ticked) — the row is still in `RailBand::rows`, still counted by the
   block's header, and still reachable by `chat_inbox` if it is asking a question; folding only
   hides its own list, and clicking the header again brings it back. This one costs no request: the
   row is already in the client.
7. It aged past `MAX_SESSIONS = 50` per agent and was swept by `prune_old` (`store/mod.rs`).
   A live session is never swept, and neither is a **starred** one.
8. It has no row in `sessions` — a leftover `<id>.log` on its own is not a session.
9. Its agent's definition was deleted — sessions are listed per *agent from the manifest list*
   (`all_agent_runs` iterates `store.list()`), so an agent with rows but no manifest is invisible to
   the UI even though `run_load` still counts it (`SessionStore::agents()`).

### `pending_question` is the one narrowing every filter yields to

A run waiting on a person — `AgentRunInfo::pending_question` (`crates/adi-webapp-api/src/types.rs`)
— is always in the rail, regardless of `SessionFilter` (Mine or Starred), the ★ agent filter, or
`hidden`, and it appears exactly once: at the top of its band with the amber dot on it, never
duplicated into the Hidden band. The server side already held it through both the page cut
(`newest`) and the `hidden` narrowing (`filter_by_hidden`, both `handlers/agents.rs`) alongside a
running or awaiting session; `source_rows` and the "Mine" retain in `actions.rs` are what carry
that same exemption through the client's own narrowings (★, "Mine"), since a subagent-launched or
unstarred run can ask a question exactly as well as one a person started by hand — and a question
nobody can see to answer is a run stuck for good.

## Hot spots for a refactor

Things that are duplicated, inconsistent, or load-bearing in a non-obvious way:

- **`Agents::list` is now the biggest single cost in the listing** — it reads all 61 agent
  *manifests* off disk, per request. Those are authored TOML and stay files by design, so the answer
  here is caching by mtime rather than another table.
- **Two lists of sessions in client state** (`all_chats` vs `watch.runs`) with a merge rule in
  `chat_all_sessions`. It exists for mutation latency, not by accident — any unification must keep
  "a deleted row leaves now, not in 3 seconds".
- **Two renderers over the same DTO** (`chat_all_sessions` and `all_chats_flatten`) that disagree
  on sort key and on hidden-filtering. `chat_hidden_sessions` is a third partial copy of the same
  flatten-and-sort.
- **`runs()` mutates.** `advance_queue` runs from the listing path. Same for `transcript()`, which
  calls `settle`. A read that writes is easy to "clean up" and break.
- **Liveness asks the *agent's current* runner** (`lib.rs:851` resolves one runner and passes it to
  `list_runs`), whereas `session_is_alive` (`lib.rs:336`) asks the *record's own* backend. They
  agree today only because all detached backends share `DetachedRunner`.
- `/api/agents/runs/all` is a full walk of every agent's session directory, at 3s, per connected
  client (deduplicated by the shared-read map, but still).
- `SessionState::Waiting` / `Error` exist in `adi-ui` and are never produced by this path — the
  wire carries no such state.
- **`(node, run_id)` is now the identity a row and an action on it carry**, not `run_id` alone —
  `SessionRow::node`, `AgentsWatch::node`, the `For` key in `chat_all_sessions`. Any code path that
  compares runs by id alone (a new feature reading `watch.run_id` without also checking `watch.node`,
  say) will quietly conflate two machines' conversations the day they happen to share one — see
  `docs/fleet.md` §13's "Multi-select" for the reasoning this is protecting.

## File index

| File | Owns |
|---|---|
| `crates/adi-agents/src/store/db.rs` | connection (thread-local), pragmas, schema |
| `crates/adi-agents/src/store/mod.rs` | `SessionStore`: list, get, create, delete, prune, queue, transcript |
| `crates/adi-agents/src/store/record.rs` | `SessionRecord`, id minting, `from_row` |
| `crates/adi-agents/src/store/transcript.rs` | `Turn`, the `turns` table, `last_activity` |
| `crates/adi-agents/src/store/queue.rs` | the `queue` table |
| `crates/adi-agents/src/store/session.rs` | `SessionRef` — the borrowed `Session` view a runner gets |
| `crates/adi-agents/src/lib.rs` | `Agents`: `runs`, `list_runs`, launch, reply, hide, delete, `settle` |
| `crates/adi-agents/src/run.rs` | `RunInfo`, `Peek`, `Launch`, `Sent` — the vocabulary |
| `crates/adi-agents/src/runner/registry.rs` | `Backend` → `Runner`, the only dispatch |
| `crates/adi-agents/src/runner/detached.rs` | `is_alive` (pid + start time), stop, event parsing |
| `crates/adi-webapp-api/src/handlers/agents.rs` | every `/api/agents/*` handler, `runs_response` |
| `crates/adi-webapp-api/src/types.rs` | `AgentRunInfo`, `AgentRuns`, `AllAgentRuns` |
| `crates/adi-app/src/main.rs` | route table, shared-read dedup |
| `crates/adi-app/src/live.rs` | the `/api/ws` watch allowlist and cadence |
| `crates/adi-webapp/src/state.rs` | `State`, `AgentsWatch`, subscription sets |
| `crates/adi-webapp/src/live.rs` | client `Sub` / `watch` |
| `crates/adi-webapp/src/fetch.rs` | the typed HTTP calls |
| `crates/adi-webapp/src/pages/agents/actions.rs` | the rail, the All-chats table, the chat screen |
| `crates/adi-ui/src/rail.rs`, `session.rs` | `Rail`, `RailGroup`, `RailCard`, `SessionItem` |
