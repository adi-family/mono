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
POST /api/agents/runs   (one agent)  GET /api/agents/runs/all  (every agent)
        │                            ╲
        ▼                              ▼
   watch.runs: Vec<AgentRunInfo>     state.all_chats: AllAgentRuns     state.rs:51, 979
        │                              │
        ╰──────────────╮   ╭───────────╯
                       ▼   ▼
        chat_all_sessions() -> Vec<SessionRow>           actions.rs:2338
        │  filter hidden, filter ★, sort by last_touch, partition running
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
<sessions_dir>/attachments/<id>         one attached image's bytes, exactly as uploaded
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
| `sessions` | the record: backend, cwd, message, `started_at`, `last_activity`, `hidden`, `runner_state`, `outcome`, `tool_help` | index `sessions_newest (agent, started_at DESC, id DESC)` |
| `turns` | one row per turn; the whole `Turn` as JSON plus `at` and `role` | `seq` |
| `queue` | what is waiting to be said next, and the images waiting with it | `seq` |
| `goals` | what the conversation is *for*: text, state, who set it, how often it has been asked | `created_at` (partial index on the open ones) |
| `attachments` | one row per uploaded image: name, media type, size, and whose message it ended up on | — |

### `attachments` is the one table that cascades off nothing

It cannot. An image is uploaded from a composer **before** the message that carries it exists —
often before the conversation does — so a new row has no session to point at, and a foreign key
would have to invent one. Its life instead runs:

1. **stored, unclaimed** — `POST /api/agents/attachment` writes the bytes to
   `<sessions_dir>/attachments/<id>` and a row with empty `agent`/`session`. The id is minted by
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

### Two ways an image reaches a model

`Runner::image_delivery` answers *how*, and the difference is where the picture ends up rather than
whether the model sees it:

| delivery | engines | what happens |
|---|---|---|
| `Inline` | `harness:adi` | the bytes go into the request body this tree writes, as base64 in each provider's own shape |
| `Path` | `harness:claude-sdk`, `process:claude`, `process:codex` | the message gets a block naming the files, and the engine opens them with its own file-reading tool |
| `None` | `pty:*`, a simulated run | nothing to send to — a pane is typed into, and a simulation has a person in the model's seat |

Path delivery is why the stored file carries its media type's **extension**: a file-reading tool
decides whether it is looking at a picture by the name, and `<id>` alone reads as text with
unprintable bytes in it. The block is appended by `for_engine` (`lib.rs`) at the moment of sending
and is deliberately **not** what the transcript records — the paths are directions for one engine,
so a reader would see plumbing under a thumbnail it is already drawing, and a turn replayed later to
a different engine would carry instructions that mean nothing to it.

A message with images to a `None` engine is **refused** (400) rather than recorded looking sent.

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
`CREATE TABLE IF NOT EXISTS` makes a no-op against an existing store.

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
| `POST /api/agents/runs` | `agent_runs` :190 | `AgentRuns` — one agent's history |
| `GET /api/agents/runs/all` | `all_agent_runs` :457 | `AllAgentRuns` — every agent, one round-trip |
| `POST /api/agents/run/peek` | `peek_run` :205 | one run's transcript/log snapshot |
| `POST /api/agents/run/hide` | `hide_run` :439 | flips `hidden`, replies with fresh history |
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
one computation.

## Layer 4 — transport

Two paths, same shape:

- **Live channel (default).** `/api/ws` re-dispatches watched reads server-side on a timer and
  pushes only when the answer changed. The allowlist *is* the security boundary
  (`adi-app/src/live.rs:74`): `/api/agents/runs/all` is `SLOW` (3s), `/api/agents/runs` and
  `/api/agents/run/peek` are `FAST` (1s).
- **Polling fallback**, when the socket is down: `adi-webapp/src/main.rs:203-250` (`refresh`),
  a 4s tick calling `fetch::all_agent_runs()` (`fetch.rs:398`).

Client subscriptions are declared in `state::chat_subscriptions` (`state.rs:1419`) and
`state::subscriptions` (`state.rs:1284`, `:1318`) and installed via `live::watch`
(`adi-webapp/src/live.rs:148`). Every write uses `set_if_changed`, so an unchanged list never
re-renders.

## Layer 5 — client state

`crates/adi-webapp/src/state.rs`. **Two signals hold session lists, and both feed the rail:**

| Signal | Source | Scope | Declared |
|---|---|---|---|
| `state.all_chats: Option<AllAgentRuns>` | `GET /api/agents/runs/all` | every agent | `state.rs:51` |
| `watch.runs: Vec<AgentRunInfo>` | `POST /api/agents/runs` | the agent on screen | `state.rs:979` |

They overlap on purpose: `watch.runs` is refreshed faster and is updated *synchronously* by
mutations (hide/delete reply with fresh history), so the row of a chat you just deleted leaves
immediately instead of at the next 3s tick. `chat_all_sessions` prefers `watch.runs` for the
watched agent and falls back to `all_chats` (`actions.rs:2377-2384`).

Other state that shapes the list: `state.starred_only`, `state.show_hidden`, `state.session_menu`,
`state.chat_drawer`.

## Layer 6 — render

`crates/adi-webapp/src/pages/agents/actions.rs`, from `chat_home_view` (`:1518`).

```
chat_rail                       :2307   the whole left rail
├─ chat_all_sessions            :2338   visible rows
│   ├─ starred_agents           :2279   ★ filter (watched agent always kept)
│   ├─ per agent: pty ⇒ one synthetic row (when: now); else runs.filter(!hidden)
│   ├─ sort by last_touch desc  :2423   last_touch = max(last_activity, started_at)  :2243
│   ├─ partition(running)       :2428   two bands
│   └─ For(keyed "agent:run_id") -> chat_session_row  :2473
└─ chat_hidden_sessions         :2674   the collapsed Hidden band (all_chats only)
```

`chat_session_row` maps a row to `adi_ui::SessionItem` (`crates/adi-ui/src/session.rs`) inside a
`RailCard` (`crates/adi-ui/src/rail.rs:38`). `SessionState` has four states but the rail only
ever produces two: `Working` when `running`, else `Done` (`actions.rs:2509`) — `Waiting` and
`Error` are unreachable from this path today.

The `For` key is `"{agent}:{run_id}"` and is load-bearing, not tidiness: a row's click handler is
bound when the row is *built*, so an unkeyed list rebuilt with a different shape (which is
exactly what toggling ★ does) opens whichever session used to sit in that slot.

### The second list

The Agents page and each project's Agents panel render a *different* list from the same data:
`all_chats_view` (`actions.rs:520`) → `all_chats_flatten` (`:542`) → `all_chats_rows` (`:577`),
a sortable table. It differs from the rail in three ways worth knowing before unifying them:

- it sorts by **`started_at`**, not `last_activity` (`:570`);
- it does **not** filter `hidden` — a workbench shows everything (`:2304-2306` explains the rule);
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
5. Next tick: the server recomputes `/api/agents/runs/all`, the answer differs, the socket pushes
   it, `state.all_chats` is set.
6. `chat_all_sessions` re-runs: the row is not hidden, its agent passes ★, `last_touch` is now,
   `running` is true → it lands at the top of the **Running now** band.
7. When the turn ends, `is_alive` goes false; the row moves to **Recent**. The answer is committed
   to the transcript by `settle` (`lib.rs:1131`) — which happens **when the chat is opened/read**,
   and deliberately stamps the turn with the log's mtime, not `now`, so committing an old answer
   does not shove that chat back to the top.

## Ordering — decided in four places

| Where | Key | Note |
|---|---|---|
| `sessions_newest` index | `started_at` desc | the store's contract; deliberately *not* activity |
| `lib.rs:879` | — | preserved, not re-sorted |
| `actions.rs:2423` (rail) | `last_touch` desc, stable | pty rows stamped `now` sort first |
| `actions.rs:570` (table) | `started_at` desc, user-sortable | disagrees with the rail on purpose |

## Why a session might not be in the list

Work down this list when one is missing:

1. Its agent's backend has **no runner** (`Backend::Other`) → `runs()` returns `[]` (`lib.rs:852`).
2. Its agent is **pty** → no history by design; the rail synthesizes one row, and only when the
   session is live or that agent is on screen (`actions.rs:2363-2375`).
3. `hidden: true` → out of the main bands, in the Hidden band (`actions.rs:2387`, `:2674`).
4. **★ is on** and its agent is not starred (`actions.rs:2279`) — off by default, so this only
   applies once someone has switched it on this page load.
5. It aged past `MAX_SESSIONS = 50` per agent and was swept by `prune_old` (`store/mod.rs:252`).
   A live session is never swept.
6. It has no row in `sessions` — a leftover `<id>.log` on its own is not a session.
7. Its agent's definition was deleted — sessions are listed per *agent from the manifest list*
   (`all_agent_runs` iterates `store.list()`), so an agent with rows but no manifest is invisible to
   the UI even though `run_load` still counts it (`SessionStore::agents()`).

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
