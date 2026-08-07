# Sessions — how a chat gets from a file on disk into the rail

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
~/.adi/mono/sessions/<agent>/<id>.*          files on disk
        │  session_ids() + record::read() + last_activity()
        ▼
SessionStore::list(agent) -> Vec<SessionRecord>          store/mod.rs:157
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

## Layer 0 — the files

Root: `~/.adi/mono/sessions/` (`Config::open()` → `adi-config/src/lib.rs:112`; the module name
is `SESSIONS_MODULE = "sessions"`, `adi-agents/src/lib.rs:79`). Override the root with `$ADI_DIR`.

```
<sessions_dir>/settings.toml                        the run cap (RunLimits)
<sessions_dir>/<agent>/<id>.meta.json               the record
<sessions_dir>/<agent>/<id>.log                     raw engine output a runner spools into
<sessions_dir>/<agent>/<id>.queue.json              messages waiting behind the live answer
<sessions_dir>/<agent>/<id>.transcript.jsonl        what was said, one turn per line
<sessions_dir>/<agent>/<id>.<anything>              runner-private sidecars
```

The id is `{unix_millis:013}-{seq:04}` (`store/record.rs:91`), so **the file name alone sorts by
start time and carries the start time** — a session whose sidecar is lost is still listed with a
real date (`store/record.rs:104`).

Note what is *not* a directory level: the backend. It used to be
(`sessions/<process|harness>/<agent>/…`), and changing an agent's backend made its whole history
vanish. Nothing sweeps the old paths forward any more — a store found in that shape needs a one-off
script, not a check on the read path.

**There is no database.** Every listing is a `read_dir` plus one `stat`/tail read per session.

## Layer 1 — `SessionStore`: files → records

`crates/adi-agents/src/store/`, entry point `SessionStore::list` (`store/mod.rs:157`).

| Step | Where | What it does |
|---|---|---|
| enumerate | `store/mod.rs:398` `session_ids` | one `read_dir`; an id counts if `<id>.meta.json` **or** `<id>.log` is present (either can exist alone) |
| read | `store/record.rs:123` `read` | parse the sidecar; a missing/corrupt/older one reads as **defaults**, never as an error |
| identity | `store/record.rs:38-45` | `id` and `agent` are `#[serde(skip_deserializing)]` — they come from the path, not the file |
| activity | `store/mod.rs:427` `last_activity` | max(last turn's `at`, `started_at`) |
| sort | `store/mod.rs:170` | **newest `started_at` first**, id as tiebreak |

Two subtleties that matter for any refactor:

- **`last_activity` is derived on every read and never written** (`store/record.rs:65`,
  `#[serde(skip)]`). It reads the *last line of the transcript* (`store/transcript.rs:181`), not
  file mtimes. That is deliberate: mtimes made a chat jump to the top of the rail merely because
  it was opened, spooled into, or had a finished answer committed. Only a message moves it.
- Reading the tail is cheap by construction: `last_line` seeks backwards in doubling windows
  (`store/transcript.rs:196`) and the result is memoized by path+mtime+len in a 64-entry LRU
  (`memo.rs:191`). A session that said nothing since the last poll costs one `stat`.

The store answers **the full history, including hidden sessions**. Hiding is a flag
(`store/mod.rs:187` `set_hidden`), never a filter — filtering is the view's job.

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

Every one of those goes through `runs_response` (`agents.rs:468`), which is where the
per-backend capability profile is attached (`agent_caps` :491 → `adi_agents::capabilities`).
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
   `RunSpec`, then `store.create(...)` (`store/mod.rs:115`) mints
   `<millis>-<seq>` and writes `<id>.meta.json`. `cwd` is pinned here, forever.
2. The opening message is appended as a user turn (`lib.rs:497`) → `<id>.transcript.jsonl`
   exists, so `last_activity` is now a real moment.
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
| `store/mod.rs:170` | `started_at` desc | the store's contract; deliberately *not* activity |
| `lib.rs:879` | — | preserved, not re-sorted |
| `actions.rs:2423` (rail) | `last_touch` desc, stable | pty rows stamped `now` sort first |
| `actions.rs:570` (table) | `started_at` desc, user-sortable | disagrees with the rail on purpose |

## Why a session might not be in the list

Work down this list when one is missing:

1. Its agent's backend has **no runner** (`Backend::Other`) → `runs()` returns `[]` (`lib.rs:852`).
2. Its agent is **pty** → no history by design; the rail synthesizes one row, and only when the
   session is live or that agent is on screen (`actions.rs:2363-2375`).
3. `hidden: true` → out of the main bands, in the Hidden band (`actions.rs:2387`, `:2674`).
4. **★ is on** and its agent is not starred (`actions.rs:2279`) — on by default.
5. It aged past `MAX_SESSIONS = 50` per agent and was swept by `prune_old` (`store/mod.rs:252`).
   A live session is never swept.
6. Neither `<id>.meta.json` nor `<id>.log` exists (`store/mod.rs:390`).
7. Its agent's definition was deleted — sessions are listed per *agent from the manifest list*
   (`all_agent_runs` iterates `store.list()`), so orphaned session directories are invisible to
   the UI even though `run_load` still counts them (`lib.rs:350`).

## Hot spots for a refactor

Things that are duplicated, inconsistent, or load-bearing in a non-obvious way:

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
- The response is **1.4 MB** for 398 sessions, because `message` carries each run's whole opening
  task. The rail truncates it anyway (`truncate_task`, `actions.rs:1497`); truncating server-side
  would cut the payload by an order of magnitude.
- `SessionState::Waiting` / `Error` exist in `adi-ui` and are never produced by this path — the
  wire carries no such state.

## File index

| File | Owns |
|---|---|
| `crates/adi-agents/src/store/mod.rs` | `SessionStore`: list, get, create, delete, prune, queue, transcript |
| `crates/adi-agents/src/store/record.rs` | `SessionRecord`, id minting, the `.meta.json` sidecar |
| `crates/adi-agents/src/store/transcript.rs` | `Turn`, the `.transcript.jsonl`, `last_at` |
| `crates/adi-agents/src/store/queue.rs` | the `.queue.json` |
| `crates/adi-agents/src/store/session.rs` | `SessionRef` — the borrowed `Session` view a runner gets |
| `crates/adi-agents/src/lib.rs` | `Agents`: `runs`, `list_runs`, launch, reply, hide, delete, `settle` |
| `crates/adi-agents/src/run.rs` | `RunInfo`, `Peek`, `Launch`, `Sent` — the vocabulary |
| `crates/adi-agents/src/runner/registry.rs` | `Backend` → `Runner`, the only dispatch |
| `crates/adi-agents/src/runner/detached.rs` | `is_alive` (pid + start time), stop, event parsing |
| `crates/adi-agents/src/memo.rs` | the LRU behind transcript reads |
| `crates/adi-webapp-api/src/handlers/agents.rs` | every `/api/agents/*` handler, `runs_response` |
| `crates/adi-webapp-api/src/types.rs` | `AgentRunInfo`, `AgentRuns`, `AllAgentRuns` |
| `crates/adi-app/src/main.rs` | route table, shared-read dedup |
| `crates/adi-app/src/live.rs` | the `/api/ws` watch allowlist and cadence |
| `crates/adi-webapp/src/state.rs` | `State`, `AgentsWatch`, subscription sets |
| `crates/adi-webapp/src/live.rs` | client `Sub` / `watch` |
| `crates/adi-webapp/src/fetch.rs` | the typed HTTP calls |
| `crates/adi-webapp/src/pages/agents/actions.rs` | the rail, the All-chats table, the chat screen |
| `crates/adi-ui/src/rail.rs`, `session.rs` | `Rail`, `RailGroup`, `RailCard`, `SessionItem` |
